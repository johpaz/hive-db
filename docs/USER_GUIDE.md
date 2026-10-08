# HiveDB — Guía de uso para agentes

> Cómo usar HiveDB desde **Bun** a través del paquete TypeScript `@johpaz/hive-db`.

---

## 1. Instalación y primer arranque

Una vez publicado en npm (ver `docs/DISTRIBUTION.md`), desde cualquier proyecto y SO:

```bash
bun add @johpaz/hive-db   # instala el binario nativo de tu plataforma automáticamente
```

Hay binarios para Linux x64 (glibc y musl/Alpine), Linux arm64, macOS x64 y arm64, y Windows x64.

Para desarrollo dentro de este monorepo:

```bash
# Dentro de un workspace Bun que ya tenga el paquete local
bun install

# Compila el binding nativo y genera native.cjs + hivedb-napi.<triple>.node
cd packages/hive-db
bun run build:native
```

Desde tu aplicación:

```ts
import { HiveDB } from "@johpaz/hive-db";

const db = await HiveDB.open("./data/my-agent");
```

`open` crea el directorio si no existe. Una vez abierta, la base de datos es un único directorio autocontenido; puedes moverlo, versionarlo (excepto el `.node`) o respaldarlo copiando la carpeta.

---

## 2. Conceptos para diseñar un agente

HiveDB modela la memoria de un agente como un **event-log inmutable** sobre el que se derivan vistas (proyecciones). En lugar de hacer `UPDATE` o `DELETE`, siempre **añades un nuevo evento**.

| Concepto | Qué representa | Ejemplo |
|---|---|---|
| `agentId` | Partición lógica de escritura. Dos agentes no se bloquean entre sí. | `"travel-agent-7"` |
| `streamId` | Sub-flujo dentro de un agente (una tarea, una conversación, un objetivo). | `"trip-to-paris"` |
| `kind` | Tipo semántico del evento. | `"Fact"`, `"StateTransition"`, `"ToolCall"` |
| `payload` | Cuerpo JSON libre con los datos del evento. | `{ "temperature": 21.5 }` |
| `seq` | Número de secuencia global asignado por el motor. Es inmutable. | `1`, `2`, `3`… |

El motor nunca te deja fijar `seq` ni `timestamp`: ambos los asigna HiveDB para garantizar orden causal y auditoría.

---

## 3. Eventos básicos: append y read

```ts
const seq = await db.append({
  agentId: "travel-agent-7",
  streamId: "trip-to-paris",
  kind: "Fact",
  payload: JSON.stringify({ temperature: 21.5 }),
});

console.log("secuencia asignada:", seq); // 1, 2, 3…

const event = await db.read(seq);
console.log(event.agentId, event.kindTag, JSON.parse(event.payload));
```

`append` devuelve el `seq` global. `read(seq)` devuelve el evento completo, incluyendo `timestamp`, `causation` y `correlation`.

### Corrección sin mutación

Si un hecho cambia, no lo editas: emites un evento `MemoryInvalidate` que apunta al `seq` del hecho anterior:

```ts
const oldFact = await db.append({
  agentId: "travel-agent-7",
  streamId: "trip-to-paris",
  kind: "Fact",
  payload: JSON.stringify({ price: 100 }),
});

await db.append({
  agentId: "travel-agent-7",
  streamId: "trip-to-paris",
  kind: "MemoryInvalidate",
  payload: JSON.stringify({ target_seq: oldFact }),
});
```

El hecho original sigue en el log (auditoría), pero las proyecciones vigentes lo ignoran.

### Métricas de herramientas y último seq

HiveDB acumula estadísticas de cada `ToolCall` automáticamente:

```ts
const seq = await db.append({
  agentId: "backend-agent",
  streamId: "task-42",
  kind: "ToolCall",
  payload: JSON.stringify({
    tool: "web_search",
    latency_ms: 120,
    cost: 0.05,
    outcome: "Ok", // "Ok" | "Timeout" | { Err: "<mensaje>" }
  }),
});

const stats = await db.toolStats("web_search");
console.log(stats?.invocations, stats?.totalLatencyMs, stats?.errors);

const last = await db.lastSeq();
console.log(last); // seq más alto asignado (puede ser mayor que `seq` si hay otras escrituras)
```

`outcome: "Timeout"` u `outcome: { Err: "<mensaje>" }` incrementan el contador `errors`. `lastSeq()` nunca escribe en disco; solo lee el contador en memoria.

---

## 4. Suscripciones reactivas

El motor puede notificarte cuando ocurre un evento que coincide con un patrón. Es útil para triggers, workflows y actualizar UI.

### Con async iterator (recomendado)

```ts
const facts = db.events({ agentId: "travel-agent-7", kind: "Fact" });

for await (const event of facts) {
  console.log("nuevo hecho:", event.seq, JSON.parse(event.payload));
  if (shouldStop(event)) break;
}

facts.close();
```

### Con callback directo

```ts
const sub = db.subscribe(
  { agentId: "travel-agent-7", kind: "Fact" },
  (event) => {
    console.log("llegó:", event.seq);
  }
);

// …más tarde
sub.close();
```

El patrón puede filtrar por `agentId`, `kind`, `streamId` y/o un predicado sobre el `payload`:

```ts
const alerts = db.events({
  kind: "Fact",
  predicate: { kind: "Eq", path: "/severity", value: "critical" },
});

const mentions = db.events({
  kind: "Fact",
  predicate: { kind: "Contains", path: "/tags", value: "billing" },
});
```

`Eq` compara por igualdad el valor en una ruta JSON pointer (`/severity` o `severity`). `Contains` comprueba que un array contenga el valor o que un string contenga la subcadena. `Always` coincide siempre. Si omites todos los campos del patrón, recibes todos los eventos.

---

## 5. Memoria semántica híbrida

HiveDB combina búsqueda full-text (BM25 sobre `tantivy`) con búsqueda vectorial (HNSW) y las fusiona con Reciprocal Rank Fusion (RRF). El análisis de texto está optimizado para español: minúsculas, plegado de acentos ("transacción" ≈ "transaccion") y stemming Snowball ("pagos" ≈ "pago"). El texto en inglés pasa casi intacto por el stemmer español, así que catálogos bilingües funcionan; lo que NO hace el motor es traducir entre idiomas ("correo" no encuentra "email" sin embeddings).

### Indexar documentos (upsert)

Cada documento tiene tres campos de texto opcionales con pesos distintos en el ranking — `name` (boost 4.0), `tags` (3.0) y `body` (2.0) — un vector opcional y filtros escalares opcionales. `upsertDoc` reemplaza el documento si el id ya existe; nunca duplica.

```ts
await db.upsertDoc({
  id: "tool:send_email",
  name: "send_email",
  body: "envía un correo electrónico al destinatario",
  tags: "comunicación email",
  filters: [{ field: "type", value: "tool" }],
});

// Lote grande: un solo commit de índice, mucho más rápido.
await db.upsertBatch(docs);
```

El vector es opcional. Hay dos formas de obtenerlo, y puedes elegir la que encaje con tu caso:

| | Embedder local (recomendado) | Vectores propios (modelo local o API) |
|---|---|---|
| Quién calcula el embedding | HiveDB, con `multilingual-e5-small` | Tú, con cualquier modelo o proveedor (OpenAI, Cohere, Voyage, un servidor propio…) |
| Configuración | `embedder: "local"` | `vector: { dimension, spaceId }` y `vector` en cada documento y consulta |
| Privacidad | El texto no sale de tu máquina | Con una API, el texto se envía a ese proveedor |
| Red | Solo la descarga única del modelo (opcional, ver abajo) | La que necesite tu proveedor |
| Coste y velocidad | Sin coste por consulta; ~47 documentos/s en CPU | Según el proveedor; suele ser más rápido en lotes grandes |

Recomendamos el embedder local porque mantiene la memoria del agente en tu máquina, pero HiveDB
funciona igual de bien con vectores de una API: el motor no distingue de dónde vienen. No se pueden
mezclar modelos en una misma base (ver `VECTOR_SPACE_MISMATCH`).

#### Embedder local (sin configurar vectores)

Con `embedder: "local"`, HiveDB genera los embeddings a partir del texto. No hace falta indicar `vector`, ni dimensión, ni `spaceId`:

```ts
const db = await HiveDB.open("./data", { embedder: "local" });

await db.upsertDoc({ id: "a", body: "Cómo configurar tu cuenta de email" });
await db.queryHybrid({ text: "correo electrónico", k: 3 }); // encuentra "a" aunque no comparta palabras
```

- **Modelo:** `intfloat/multilingual-e5-small` (licencia MIT, 384 dimensiones, español, inglés y más). El resultado coincide con el modelo ONNX oficial a ~1e-7 por componente.
- **Primera apertura:** descarga el modelo (~470 MB) a `~/.cache/hivedb/models` (cambia la ruta con `HIVEDB_MODEL_DIR`). Se verifica con SHA-256 y no se vuelve a descargar. Con `HIVEDB_OFFLINE=1` nunca accede a la red y falla con `EMBEDDER_UNAVAILABLE` si falta el modelo. Esta descarga es la **única excepción** al principio de «cero servicios externos»: es opcional (solo con `embedder: "local"`), puntual y con la revisión del modelo fijada; ver «Instalaciones sin red».
- **Requiere un binario con el embedder incluido** (feature de Cargo `embedder-local`, +6 MB). Si no, `open` falla con `EMBEDDER_UNAVAILABLE`.
- **Cómo cambia el comportamiento:** los documentos sin `vector` pero con texto se embeben (`name`, `tags` y `body`); una consulta de texto sin `vector` también busca por significado, así que sus puntuaciones pasan a ser RRF y no BM25 puro. Un `vector` que aportes tú siempre tiene prioridad.
- **Rendimiento (CPU, 16 núcleos):** ~45–60 documentos/s al indexar (frases de Wikipedia de ~130 caracteres: ~47/s) y ~50 ms por consulta de texto. Los textos de un lote se procesan por longitud para no rellenar de más. Indexar corpus grandes es lento; para eso puedes aportar tus propios vectores.
- **Cambiar de modelo:** una base queda ligada al modelo con que se creó; abrirla con otro falla con `VECTOR_SPACE_MISMATCH`.

#### Instalaciones sin red (air-gapped)

El modelo se puede descargar de antemano y llevarlo a una máquina sin acceso a internet:

```bash
# 1. En una máquina con red: descargar y verificar el modelo en un directorio
HIVEDB_MODEL_DIR=./modelo cargo run --release -p hivedb-embed --example fetch_model

# 2. Copiar ./modelo (contiene multilingual-e5-small-614241f6/) a la máquina aislada.

# 3. En la máquina aislada: apuntar a ese directorio y prohibir la red
export HIVEDB_MODEL_DIR=/ruta/a/modelo
export HIVEDB_OFFLINE=1
```

Los tres ficheros del modelo, con las sumas SHA-256 que el motor comprueba al descargar:

| Fichero | Bytes | SHA-256 |
|---|---:|---|
| `config.json` | 655 | `69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959` |
| `tokenizer.json` | 17 082 730 | `0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39` |
| `model.safetensors` | 470 641 600 | `1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477` |

Al arrancar el motor acepta un fichero ya presente por su tamaño; si lo has copiado a mano, comprueba
las sumas con `sha256sum`.

El modelo **nunca se distribuye dentro del binario ni de un paquete**: se descarga cuando activas el
embedder local (`embedder: "local"`), o de antemano con el procedimiento anterior. Quien no activa el
embedder no descarga nada ni paga su tamaño.

#### Aportar tus propios vectores

Para usar tu propio modelo —local o de una API— debes declarar explícitamente un espacio estable al abrir; omitir `vector` deja la base en modo texto. HiveDB no llama a ningún proveedor por ti: calculas el embedding y se lo pasas:

```ts
const db = await HiveDB.open("./data", {
  vector: {
    dimension: 768,
    spaceId: "my-embedding-space-v1",
  },
});
```

Documentos y consultas deben producirse con el mismo modelo y configuración representados por `spaceId`. HiveDB rechaza dimensiones distintas, NaN, infinitos y vectores de norma cero.

**Actualizar desde 0.3.x.** Una base creada con 0.3.x abre sin cambios en tu código: su `meta.json` se migra solo y colecciones y event-log quedan intactos. Lo que no sobrevive es el índice semántico —0.3.x no guardaba los documentos en ningún lugar del que se puedan recuperar—, así que después de actualizar vuelve a indexar tus documentos con `upsertBatch`. Si necesitas volver a 0.3.x, la base migrada sigue abriéndose con esa versión.

**Actualizar desde 0.5.x.** Las versiones posteriores a 0.5.x guardan los vectores en un fichero plano
(`vectors.0.dat`) en vez de dentro de cada documento, lo que reduce el disco a la mitad. Al abrir una
base de 0.5.x se migra sola (por tandas, reanudable si se interrumpe) y `meta.json` no se modifica.
Una vez migrada **no se puede abrir con 0.5.x**: esa versión falla con un error de tipo en la tabla
`semantic_docs` en lugar de ver un índice vacío. Haz una copia del directorio antes de actualizar si
quieres conservar la posibilidad de volver atrás. Los vectores se guardan normalizados (la métrica es
coseno), así que no se conserva su magnitud original.

### Rendimiento, precisión y disco

Medido con 100.000 documentos de 384 dimensiones (detalle en [`BENCHMARKS.md`](BENCHMARKS.md)):

| | |
|---|---|
| Búsqueda vectorial | ~0,6–0,8 ms (p50), ~1,2–1,6 ms (p99), recall@10 ≈ 0,996 con el `efSearch` por defecto |
| Búsqueda híbrida (texto + vector) | ~1,6–1,9 ms (p50) |
| Abrir una base ya poblada | ~40 ms (el grafo y el índice de texto se restauran del disco) |
| Cerrar | ~50 ms |
| Disco | ~204 MiB (146 MiB de vectores, 33 MiB de documentos, 20 MiB de grafo, 4 MiB de texto) |
| Inserción por lotes | ~8 000 documentos/s (usa `upsertBatch`: enlaza el grafo en paralelo) |

El grafo ANN es HNSW. Cada consulta puede ajustar el equilibrio entre precisión y velocidad con
`efSearch` (por defecto 200); más alto es más preciso y más lento:

```ts
await db.queryHybrid({ vector, k: 10, efSearch: 100 }); // más rápido, algo menos preciso
await db.queryHybrid({ vector, k: 10, efSearch: 800 }); // más preciso
```

| `efSearch` | recall@10 | Vector p50 (100k docs) |
|---:|---:|---:|
| 50 | 0,87 | 0,4 ms |
| 100 | 0,975 | 0,5 ms |
| **200** (defecto) | 0,996 | 0,6 ms |
| 400 | 0,997 | 1,0 ms |
| 800 | 0,998 | 1,4 ms |

Estas cifras son con vectores sintéticos agrupados; con tus embeddings el recall puede variar, así
que mide con tus datos antes de bajar `efSearch`. Para indexar muchos documentos usa `upsertBatch`
en lugar de `upsertDoc` en un bucle: es mucho más rápido.

Los vectores se guardan una sola vez, en un fichero plano (`vectors.0.dat`), y **normalizados**: la
métrica es coseno, así que no se conserva su magnitud original. Actualizar o borrar un documento deja
su vector antiguo como espacio muerto; cuando es el 25 % o más (y al menos 1.024 vectores) se
compacta solo, o puedes forzarlo con `compactIndex()`.

### Consultar

```ts
const hits = await db.queryHybrid({
  text: "¿cómo genero reportes de transacciones?", // texto crudo: nunca lanza error
  k: 5,
  filters: [{ field: "type", value: "tool" }],
  boosts: { name: 4, tags: 3, body: 2 }, // opcional
});

for (const hit of hits) {
  console.log(hit.id, hit.score);
}
```

Puedes consultar solo por texto, solo por vector, o ambos. La semántica del `score` depende del modo:

| Modo | `score` |
|---|---|
| Solo texto | BM25 crudo (positivo, mayor = mejor) |
| Solo vector | Similitud coseno (-1 a 1) |
| Híbrido | Fusión RRF (`fusion: { kind: "rrf", k: 60 }` configurable); `textScore` y `vectorScore` traen los componentes crudos |

El parsing del texto es tolerante: comillas sin cerrar, operadores y signos de puntuación degradan a una búsqueda bolsa-de-palabras en lugar de fallar.

### Borrar y mantener

```ts
await db.deleteDoc("tool:send_email");                       // por id
await db.deleteByFilter({ field: "server_id", value: "a" }); // por filtro (p. ej. hot-reload MCP)
await db.clearIndex();                                       // vaciar todo el índice
await db.compactIndex();                                     // reescribir los vectores sin espacio muerto y reconstruir los índices derivados
```

`HiveDB.open(":memory:")` abre una base efímera (ideal para tests) con el índice semántico completo.

---

## 6. Consentimiento y autorización

HiveDB incluye un grafo de consentimiento. Un agente puede delegar permisos a otro, revocarlos, y luego consultar si una acción está autorizada. Las delegaciones pueden encadenarse transitivamente: si `owner → lead` y `lead → worker`, entonces `worker` está autorizado; la cadena se invalida si cualquier eslabón expira o se revoca.

### Otorgar consentimiento

```ts
await db.append({
  agentId: "owner",
  streamId: "consent",
  kind: "ConsentGranted",
  payload: JSON.stringify({
    from: "owner",
    to: "assistant",
    scope: { action: "read", resource: "trips/*" },
    expires: Date.now() + 24 * 60 * 60 * 1000, // opcional, epoch ms
  }),
});
```

### Revocar consentimiento

```ts
await db.append({
  agentId: "owner",
  streamId: "consent",
  kind: "ConsentRevoked",
  payload: JSON.stringify({ grant_seq: 1 }),
});
```

### Consultar autorización

```ts
const decision = await db.can("assistant", "read", "trips/paris");
console.log(decision.allowed); // true | false
console.log(decision.intentLogSeq); // seq del evento IntentLogged de auditoría
```

Cada llamada a `can` genera automáticamente un evento `IntentLogged` en el log para dejar traza auditable.

---

## 7. Colecciones de documentos (CRUD mutable)

A diferencia del event-log (append-only), las colecciones son un almacén de documentos JSON mutable con `put`/`get`/`delete`/`scan` — la pieza que falta para reemplazar tablas relacionales estilo SQLite sin salir de HiveDB.

```ts
interface Agent {
  name: string;
  role: string;
}

const agents = db.collection<Agent>("agents");

const version = await agents.put("a1", { name: "Atlas", role: "worker" }); // 1
const entry = await agents.get("a1"); // { id: "a1", version: 1, doc: { name: "Atlas", role: "worker" } }
await agents.delete("a1"); // true si existía
```

### Versionado optimista

`put` acepta `expectedVersion` para evitar pisar cambios concurrentes. `expectedVersion: 0` significa "crear solo si no existe". Un desajuste lanza (`version conflict`).

```ts
await agents.put("a1", { name: "Atlas", role: "worker" }, { expectedVersion: 0 }); // crea
await agents.put("a1", { name: "Atlas", role: "coordinator" }, { expectedVersion: 1 }); // actualiza
await agents.put("a1", { name: "Atlas", role: "coordinator" }, { expectedVersion: 1 }); // lanza: ya está en versión 2
```

### Scan

```ts
await agents.scan({ prefix: "a", limit: 20, reverse: false });
await agents.count();
```

Recorre por orden lexicográfico de `id`; `prefix` filtra, `offset`/`limit` paginan, `reverse` invierte el orden.

### Índices secundarios

`createIndex` indexa un campo escalar (string/number/bool) del documento y hace backfill de los documentos existentes. `unique: true` rechaza duplicados (en el `put` y también durante el backfill). Campos ausentes o no-escalares (arrays, objetos) se omiten del índice en vez de fallar.

```ts
await agents.createIndex("role");
const workers = await agents.findBy("role", "worker"); // DocEntry[]

await agents.createIndex("email", { unique: true });
await agents.put("a2", { email: "dup@x.com" }); // ok
await agents.put("a3", { email: "dup@x.com" }); // lanza: unique index violado
```

`findBy` sobre un campo sin índice creado lanza explícitamente — no hay full-scan implícito.

### Batches atómicos

`db.batch(ops)` aplica varias operaciones (incluso en distintas colecciones) en una sola transacción: o se aplican todas, o ninguna.

```ts
await db.batch([
  { op: "put", collection: "agents", id: "a1", doc: { name: "Atlas" } },
  { op: "put", collection: "agents", id: "a2", doc: { name: "Nova" }, expectedVersion: 3 },
  { op: "delete", collection: "logs", id: "stale-1" },
]);
```

Si cualquier operación falla (por ejemplo un `expectedVersion` desactualizado), nada del batch se commitea.

---

## 8. Memoria de trabajo (Working Memory)

Para datos transitorios que no necesitan durabilidad de log pero sí rápido acceso con TTL:

```ts
await db.workingSet("agent-1", "draft", { text: "borrador" });
const draft = await db.workingGet("agent-1", "draft");
console.log(draft); // { text: "borrador" }

// Con expiración (TTL en milisegundos)
await db.workingSet("agent-1", "temp", 42, 1000);

// Listar claves almacenadas por agente
const keys = await db.workingKeys("agent-1");
```

La memoria de trabajo no escribe en el event-log: es puramente efímera y se pierde al cerrar la base de datos.

---

## 9. Harness de larga duración

HiveDB puede reconstruir el hilo causal de una tarea (`streamId`) y generar ventanas de contexto adaptativas para LLMs, además de evaluar el proceso del agente.

### Hilo causal

```ts
const thread = (await db.causalThread("task-1")) as any;
console.log(thread.decisions.length, thread.toolCalls.length);
```

El hilo sigue los enlaces `causation` entre decisiones y llamadas a herramientas, y detecta bucles de error y deriva de objetivo.

### Contexto adaptativo

```ts
const ctx = (await db.buildAgentContext({
  taskId: "task-1",
  currentPhase: "implementation",
  currentObjective: "fix payment module",
  maxTokens: 4096,
  strategy: {
    causalAnchors: true,
    compressCompletedPhases: true,
  },
})) as any;
```

El motor garantiza que el contexto resultante nunca exceda `maxTokens`, comprime fases terminadas, mantiene anclas causales y puede incluir episodios similares. Cada item de `ctx.items` trae un campo `type` (`"decision"` / `"toolCall"` / `"anomaly"` / `"episode"` / `"phaseSummary"`) para distinguir la variante.

### Evaluación del proceso

```ts
const evalResult = (await db.evaluateHarness({
  causalThread: thread,
  similarEpisodes: [],
  originalIntent: "implementar autenticación",
  currentState: { outcome: "success" },
  minConfidence: 0.5,
})) as any;

console.log(evalResult.processQuality, evalResult.outputQuality);
console.log(evalResult.proposals);
```

La evaluación es pura: recibe datos y devuelve `processQuality`, `outputQuality`, `rootCause`, `findings` y `proposals` con evidencia causal.

---

## 10. Cierre y ciclo de vida

```ts
db.close();
```

`close` libera el handle nativo. Las suscripciones abiertas se cancelan automáticamente al cerrar.

Si necesitas reconstruir las proyecciones desde cero (por ejemplo tras añadir una nueva proyección en el motor), usa el comando correspondiente en Rust; no está expuesto en TS por ser una operación de mantenimiento.

---

## 11. Patrones recomendados para agentes

1. **Un `agentId` por agente autónomo.** No compartas `agentId` entre instancias concurrentes si no quieres contención de escritura.
2. **Un `streamId` por objetivo o conversación.** Facilita leer la historia completa de una tarea.
3. **No uses el log como cola de trabajo de alta frecuencia.** Las suscripciones reactivas son *at-least-once*; para trabajo crítico usa confirmaciones idempotentes.
4. **Indexa documentos justo después de append.** Así textos y vectores quedan disponibles para búsqueda inmediata.
5. **Mide RSS si abres/cerras muchas bases.** Cada `HiveDB.open` carga índices en memoria; reutiliza una instancia compartida cuando sea posible.

---

## 12. Referencia rápida de tipos

```ts
interface EventInput {
  agentId: string;
  streamId: string;
  kind: "Fact" | "StateTransition" | "MemoryInvalidate" | "ToolCall" | "ConsentGranted" | "ConsentRevoked" | "IntentLogged" | "LearningProposal";
  payload: string; // JSON string
  causation?: number; // seq del evento que causó este evento
}

interface Event {
  seq: number;
  agentId: string;
  streamId: string;
  kindTag: string;
  timestamp: number;
  causation?: number;
  correlation?: string;
  payload: string;
}

interface IndexDoc {
  id: string;
  name?: string;   // boost 4.0
  body?: string;   // boost 2.0
  tags?: string;   // boost 3.0
  vector?: Float32Array; // requiere configuración vectorial explícita al abrir
  filters?: ScalarFilter[];
}

interface HybridQuery {
  text?: string;
  vector?: Float32Array;
  k: number;
  filters?: ScalarFilter[];
  fusion?: { kind: "rrf"; k?: number };
  boosts?: { name?: number; body?: number; tags?: number };
  efSearch?: number; // precisión/velocidad del ANN (por defecto 200)
}

interface ScalarFilter {
  field: string;
  value: string;
}

interface Hit {
  id: string;
  score: number;       // BM25 | coseno | RRF según el modo
  textScore?: number;
  vectorScore?: number;
}

interface Decision {
  allowed: boolean;
  intentLogSeq?: number;
}

interface DocEntry<T = unknown> {
  id: string;
  version: number;
  doc: T;
}

interface PutDocOptions {
  expectedVersion?: number; // 0 = crear solo si no existe
}

interface ScanOptions {
  prefix?: string;
  start?: string;
  offset?: number;
  limit?: number;
  reverse?: boolean;
}

type BatchOp =
  | { op: "put"; collection: string; id: string; doc: unknown; expectedVersion?: number }
  | { op: "delete"; collection: string; id: string };

class Collection<T = unknown> {
  put(id: string, doc: T, options?: PutDocOptions): Promise<number>; // -> nueva versión
  get(id: string): Promise<DocEntry<T> | undefined>;
  delete(id: string): Promise<boolean>;
  scan(options?: ScanOptions): Promise<DocEntry<T>[]>;
  count(): Promise<number>;
  createIndex(field: string, options?: { unique?: boolean }): Promise<void>;
  findBy(field: string, value: string | number | boolean): Promise<DocEntry<T>[]>;
}

class HiveDB {
  static open(path: string, options?: {
    vector?: { dimension: number; spaceId: string };
  }): Promise<HiveDB>;
  append(input: EventInput): Promise<number>;
  read(seq: number): Promise<Event>;
  logLen(): Promise<number>;
  can(agent: string, action: string, resource: string): Promise<Decision>;
  upsertDoc(doc: IndexDoc): Promise<void>;
  upsertBatch(docs: IndexDoc[]): Promise<void>;
  deleteDoc(id: string): Promise<void>;
  deleteByFilter(filter: ScalarFilter): Promise<void>;
  clearIndex(): Promise<void>;
  indexDoc(id: string, text: string, vector: Float32Array, filters?: ScalarFilter[]): Promise<void>; // deprecado
  compactIndex(): Promise<void>;
  queryHybrid(query: HybridQuery): Promise<Hit[]>;
  events(pattern: EventPattern): AsyncIterable<Event> & { close(): void };
  subscribe(pattern: EventPattern, onEvent: (event: Event) => void): Subscription;
  collection<T = unknown>(name: string): Collection<T>;
  batch(ops: BatchOp[]): Promise<void>;
  causalThread(streamId: string): Promise<unknown>;
  buildAgentContext(req: AgentContextRequest): Promise<unknown>;
  evaluateHarness(input: HarnessInput): Promise<unknown>;
  close(): void;
}

interface AgentContextRequest {
  task_id: string;
  current_phase: string;
  current_objective: string;
  max_tokens: number;
  strategy: ContextStrategy;
}

interface ContextStrategy {
  causal_anchors?: boolean;
  compress_completed_phases?: boolean;
  episodic_similarity?: { vector: Float32Array; k: number };
  recent_anomalies?: { window_ms: number };
}

interface HarnessInput {
  causal_thread: unknown;
  similar_episodes?: unknown[];
  original_intent?: string;
  current_state?: unknown;
  min_confidence?: number;
}
```
