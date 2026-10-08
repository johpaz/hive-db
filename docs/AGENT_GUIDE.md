# HiveDB para agentes: para qué sirve y cómo usarlo bien

Esta guía es para quien construye un runtime de agentes (hive, hiveCode, hive-sdk, HiveLearn…) y
quiere aprovechar HiveDB a fondo, no solo guardar y leer filas. No repite la referencia de cada
API (eso está en [`USER_GUIDE.md`](USER_GUIDE.md)): explica **qué problema resuelve cada pieza,
cuándo usarla, cómo diseñar la memoria de un agente y qué errores evitar**.

> **Estado.** Todo lo que sigue funciona hoy salvo lo marcado como *pendiente*. Los paquetes
> publicados (desde la 0.6.0) ya incluyen el embedder local; el modelo (~470 MB) se descarga al
> activarlo, y conviene precargarlo con `HiveDB.prepareEmbedder` (ver §4.4).

---

## 1. Qué es HiveDB y para qué sirve

HiveDB es la memoria de un agente, embebida en su proceso y guardada en un directorio. Reúne en un
solo motor cuatro cosas que un agente suele acabar montando con piezas sueltas:

| Pregunta que se hace el agente | Pieza de HiveDB | Ejemplo |
|---|---|---|
| «¿Qué pasó y por qué?» | **Event-log** inmutable con causalidad | Decisiones, llamadas a herramientas, errores, intenciones |
| «¿Cuál es el estado ahora?» | **Colecciones** mutables con versionado | Configuración, sesiones, mensajes, agentes |
| «¿Qué sé que sea relevante para esto?» | **Memoria semántica** híbrida (palabras + significado) | Herramientas, skills, reglas aprendidas, resúmenes, recuerdos |
| «¿Puedo hacer esto?» | **Consentimiento** con auditoría | Delegaciones con alcance y expiración |

Más: memoria de trabajo con TTL, suscripciones push y un *harness* que reconstruye el hilo
causal de una tarea y arma ventanas de contexto para el LLM.

**Lo que no es:** una cola de trabajos, un orquestador, un extractor de recuerdos con LLM (como
Mem0) ni un servidor. Tu aplicación decide qué guardar y cuándo; HiveDB lo guarda, lo ordena y lo
recupera rápido.

---

## 2. Qué va en cada sitio

La decisión de diseño más importante es **elegir bien dónde se guarda cada cosa**.

| Si lo que guardas es… | Úsalo en… | Porque… |
|---|---|---|
| Un hecho que ocurrió y no debe cambiar (una decisión, una llamada, un error) | **Event-log** (`append`) | Es inmutable y ordenado por el motor; sirve para auditar y para el análisis causal |
| Estado que cambia (config, sesión, perfil, el último resumen) | **Colección** (`collection(...).put`) | Se actualiza en sitio, con versionado optimista e índices secundarios |
| Conocimiento que quieres **encontrar por texto o significado** | **Índice semántico** (`upsertDoc` / `upsertBatch` + `queryHybrid`) | Búsqueda híbrida con filtros exactos |
| Algo transitorio (borrador del turno, caché) | **Memoria de trabajo** (`workingSet`) | RAM con TTL; no toca el disco |
| Una autorización | **Consentimiento** (`ConsentGranted` + `can`) | Con expiración, revocación y rastro de auditoría |

Una regla práctica: **el log dice qué pasó, las colecciones dicen cómo está ahora, el índice
dice qué es relevante.** Un mismo dato puede estar en dos sitios con funciones distintas (por
ejemplo, un mensaje en una colección para reconstruir la conversación, y su resumen en el índice
para recuperarlo por significado).

---

## 3. Reglas de oro

1. **Un único proceso es dueño de cada base.** HiveDB abre en exclusiva: un segundo proceso que
   abra el mismo directorio falla con «Database already open». Diseña un proceso propietario (el
   *gateway*) y que el resto hable con él por IPC. En tests, usa un directorio temporal por
   suite o `":memory:"`.
2. **Abre una vez, cierra limpio.** Mantén una instancia compartida y llama a `close()` al
   terminar. Un cierre limpio deja guardados el grafo y el índice de texto, y la siguiente
   apertura tarda decenas de milisegundos; tras un cierre brusco todo se reconstruye solo (es
   seguro, pero más lento).
3. **No guardes secretos ni datos personales en el log.** Es inmutable: lo que se añade no se
   puede borrar. Guarda una referencia (un id) y deja el secreto en un almacén aparte.
4. **Ids estables y con prefijo de tipo** (`tool:web_search`, `skill:refactor`, `episode:9f3…`).
   Un `upsert` con el mismo id reemplaza; un id inestable duplica.
5. **Particiona con filtros, no con bases distintas** salvo que necesites aislamiento fuerte.
   Un campo `type`, `tenant`, `project`… en `filters` permite consultar un ámbito concreto.
   Ten en cuenta que `deleteByFilter` acepta **un solo** filtro: si necesitas borrar «por tipo
   dentro de un inquilino», indexa un campo combinado (hive-sdk usa `tenant__type`).
6. **Indexa por lotes.** `upsertBatch` enlaza el grafo en paralelo y confirma una sola vez;
   `upsertDoc` en bucle es varias veces más lento.
7. **No reindexes lo que no ha cambiado.** Guarda un hash del contenido (en una colección) y haz
   `upsert` solo de lo que cambió. Con embeddings esto importa: reindexar ~120 documentos de
   catálogo en cada arranque son ~3 s de CPU con el embedder local, o dinero con una API.
8. **Escribe el texto en lenguaje natural**, no en JSON ni en código crudo: es lo que se
   tokeniza y se embebe.

---

## 4. Memoria semántica para agentes

### 4.1 Qué indexar

| Qué | Cómo (id · filtros) | Para qué |
|---|---|---|
| Herramientas, skills, servidores MCP | `tool:x` · `type=tool` | Elegir qué herramientas ofrecer al modelo en cada turno |
| Reglas aprendidas (playbook) | `playbook:n` · `type=playbook` | Inyectar solo las reglas aplicables |
| Resúmenes de conversación | `summary:thread:n` · `type=summary`, `thread=…` | Recuperar contexto antiguo sin reenviar todo el historial |
| **Episodios** (qué se intentó, cómo acabó) | `episode:id` · **`kind=episode`** | Aprender de casos parecidos; los usa `buildAgentContext` |
| Hechos del proyecto o del usuario | `fact:…` · `type=fact`, `project=…` | Memoria a largo plazo |

Qué **no** indexar: cada mensaje en bruto de cada conversación (ruido y coste), resultados de
herramientas enormes (indexa un resumen) y nada que no quieras que un modelo pueda recuperar.

### 4.2 Cómo escribir un documento

```ts
await db.upsertBatch([
  {
    id: "tool:send_email",
    name: "send_email",                        // más peso (×4)
    tags: "correo email enviar mensaje",       // peso medio (×3)
    body: "Envía un correo electrónico a uno o varios destinatarios.", // (×2)
    filters: [{ field: "type", value: "tool" }],
  },
]);
```

`name`, `tags` y `body` se indexan por texto con pesos distintos; con embedder, los tres se
combinan para generar el vector. Los **filtros** son campos exactos y se combinan con AND.

### 4.3 Cómo consultar

```ts
const hits = await db.queryHybrid({
  text: "mandar un correo al cliente",
  k: 8,
  filters: [{ field: "type", value: "tool" }],
});
for (const h of hits) console.log(h.id, h.score, h.textScore, h.vectorScore);
```

- Solo texto → `score` es BM25 (positivo, más alto = mejor). Solo vector → similitud coseno.
  Texto + vector → `score` es **RRF** (una puntuación basada en posiciones, de orden ~0,01–0,03);
  `textScore` y `vectorScore` traen los valores crudos.
- Con **filtros y vector**, el motor calcula el coseno exacto sobre los documentos que cumplen el
  filtro (no aproxima): resultados completos aunque el filtro sea muy selectivo.
- Un texto mal formado nunca lanza error: degrada a bolsa de palabras.

> **Cuidado al pasar de solo-BM25 a híbrido.** Un umbral relativo sobre `score` (como el
> `applyRelativeCutoff` de hive-sdk) deja de significar lo mismo con RRF, porque ya no es BM25. Para
> filtrar por relevancia usa `vectorScore` o `textScore`, y **calibra el umbral con tus datos**.
> Los embeddings de la familia E5 concentran las similitudes en un rango alto (típicamente
> 0,7–1,0), así que un corte absoluto bajo no descarta nada: usa el corte relativo al mejor
> resultado o mide con un conjunto de consultas reales.

### 4.4 Qué embeddings usar

| | Embedder local (**recomendado**) | Vectores propios (modelo local o API) |
|---|---|---|
| Activación | `HiveDB.open(path, { embedder: "local" })` | `HiveDB.open(path, { vector: { dimension, spaceId } })` y `vector` en cada documento y consulta |
| Privacidad | El texto no sale de la máquina | Con una API, el texto va al proveedor |
| Coste | Ninguno por consulta | Según el proveedor |
| Velocidad | ~47 documentos/s indexando, ~50 ms por consulta de texto (CPU) | Depende del proveedor |
| Idiomas | Español, inglés y más (`multilingual-e5-small`) | Lo que tenga tu modelo |

Decisiones que se toman **una vez por base**:
- La base queda ligada a un modelo (`spaceId`). Abrirla con otro falla con `VECTOR_SPACE_MISMATCH`.
  Para cambiar de modelo hay que crear una base nueva o vaciar el índice y reindexar todo.
- El vector se guarda **normalizado** (la métrica es coseno): no se conserva su magnitud.
- La descarga del modelo (~470 MB, la primera vez que activas el embedder local) es la única
  conexión de red del motor. Hazla antes y con progreso: `await HiveDB.prepareEmbedder({ onProgress })`
  (con reintentos y reanudación; es idempotente, llámala siempre al arrancar). Para máquinas sin red, ver la sección «Instalaciones sin red» de
  [`USER_GUIDE.md`](USER_GUIDE.md).

### 4.5 Rendimiento: lo que conviene saber

Con 100.000 frases reales de 384 dimensiones ([`BENCHMARKS.md`](BENCHMARKS.md)): búsqueda vectorial
~1,4 ms, texto ~0,8 ms, híbrida ~2,8 ms, apertura ~40 ms, ~5.200 documentos/s al indexar por lotes y
~241 MiB en disco. Un catálogo de agente (cientos de documentos) es, por tanto, trivial para el
motor; **lo que cuesta de verdad es calcular el embedding**, no buscarlo:

- La consulta de texto con embedder local cuesta ~50 ms de CPU por embeber la frase. Si en cada
  turno haces varias consultas con el mismo texto, **embebe una vez** y reutiliza el resultado (o
  cachea las consultas recientes en tu aplicación).
- `efSearch` (por defecto 200) ajusta precisión y velocidad del grafo; con catálogos pequeños no
  hace falta tocarlo.

---

## 5. Recetas

### 5.1 Un proceso propietario

```ts
// storage/hivedb.ts — una instancia compartida
let db: HiveDB | null = null;
let opening: Promise<HiveDB> | null = null;

export async function getHiveDb(): Promise<HiveDB> {
  if (db) return db;
  opening ??= HiveDB.open(path, { embedder: "local" }).then((d) => (db = d));
  return opening;
}
export function closeHiveDb() { db?.close(); db = null; opening = null; }
// process.on("SIGTERM", closeHiveDb) — cierre limpio
```

### 5.2 Registrar un turno de forma que el harness lo entienda

El análisis causal solo funciona si los eventos tienen la forma que espera. Resumen (el contrato
completo está en [`AGENT_INTEGRATION.md`](AGENT_INTEGRATION.md)):

```ts
// 1. La intención del usuario ancla la tarea.
const intent = await db.append({
  agentId: "coder", streamId: "task-42", kind: "IntentLogged",
  payload: JSON.stringify({ actor: "user", intent: "arreglar el módulo de pagos" }),
});

// 2. Cada decisión, enlazada a su causa.
const decision = await db.append({
  agentId: "coder", streamId: "task-42", kind: "StateTransition",
  payload: JSON.stringify({ description: "leer el módulo y reproducir el fallo", phase: "diagnóstico" }),
  causation: intent,
});

// 3. Cada herramienta, con `outcome` canónico: "Ok" | "Timeout" | { Err: "…" }.
await db.append({
  agentId: "coder", streamId: "task-42", kind: "ToolCall",
  payload: JSON.stringify({ tool: "bash", latency_ms: 840, outcome: { Err: "exit 1" } }),
  causation: decision,
});
```

Un `outcome` con otra forma (`"error"`, `false`…) se cuenta como éxito: **el fallo se pierde en
silencio**. Sin `causation` no hay grafo causal; sin `IntentLogged` no se detecta deriva del
objetivo.

### 5.3 Recuperar contexto antes de cada turno

```ts
// Reglas y resúmenes relevantes para el mensaje del usuario
const [reglas, recuerdos] = await Promise.all([
  db.queryHybrid({ text: mensaje, k: 5, filters: [{ field: "type", value: "playbook" }] }),
  db.queryHybrid({ text: mensaje, k: 3, filters: [{ field: "type", value: "summary" }, { field: "thread", value: hilo }] }),
]);
```

Los filtros hacen que cada consulta mire solo su ámbito. Dos consultas con filtros distintos son
dos llamadas baratas; no intentes una sola con «OR».

### 5.4 Contexto adaptativo para tareas largas

`buildAgentContext` construye la ventana que se envía al modelo a partir del hilo causal: nunca
supera `maxTokens`, comprime las fases terminadas y mantiene las decisiones ancladas a la intención.

```ts
const ctx = await db.buildAgentContext({
  taskId: "task-42",
  currentPhase: "implementación",
  currentObjective: "arreglar el módulo de pagos",
  maxTokens: 4096,
  strategy: { causalAnchors: true, compressCompletedPhases: true, episodicSimilarity: true },
});
```

`episodicSimilarity` solo devuelve algo si has indexado **episodios** con el filtro `kind=episode`
(§4.1); sin ese campo, la lista sale vacía sin error.

### 5.5 Pedir permiso antes de una acción sensible

```ts
const d = await db.can("assistant", "write", "workspace/pagos");
if (!d.allowed) return rechazar();
```

Cada llamada a `can` deja un evento de auditoría. Concede permisos con `ConsentGranted` (con
`expires`) y revócalos con `ConsentRevoked`.

### 5.6 Varios inquilinos en una sola base

Es el patrón de Hive Cloud: una base, y el ámbito como filtro (`tenant`) más un prefijo en los ids
de las colecciones. Funciona bien mientras: (a) **todas** las consultas lleven el filtro de
ámbito, (b) los borrados masivos se acoten con un campo combinado, y (c) aceptes que un fallo de
ámbito en tu código mezcla datos. Si necesitas aislamiento fuerte (datos regulados), una base por
inquilino es más segura; abrir una base ya cuesta decenas de milisegundos, pero cada base es un
directorio con su propio bloqueo exclusivo. Con el embedder local, ver además §5.7.

### 5.7 Muchos usuarios o enjambres con el embedder local: un modelo por aplicación

Cuando un SDK de agentes (hive-sdk, un servicio con LangGraph.js o LangChain.js…) usa HiveDB como
memoria y cada usuario o enjambre tiene su espacio, **el modelo de embeddings es de la aplicación, no
de cada usuario**:

- **Se descarga una vez por máquina,** no por usuario ni por enjambre: vive en `HIVEDB_MODEL_DIR` (o en
  la caché del usuario del sistema). En un servidor, fija `HIVEDB_MODEL_DIR` a un directorio común y
  llama a `HiveDB.prepareEmbedder()` al arrancar.
- **Se carga una sola vez por proceso.** Todas las bases abiertas con `embedder: "local"` comparten la
  misma instancia del modelo. Medido: ~750 MiB para la primera base (el modelo) y **~12 MiB por cada base
  adicional**; con 4 bases, 813 MiB en total (antes de la 0.6.1 cada base cargaba su propia copia: 2,9 GB).
- **Las consultas concurrentes no se serializan.** Varias consultas a la vez, sobre la misma base o sobre
  bases distintas, se reparten por los núcleos: 16 consultas simultáneas con embedding tardan 119 ms en
  total (≈ 130 consultas/s en 16 hilos), frente a 742 ms en fila. El coste de CPU de cada consulta de texto
  es lo que limita el caudal.

Dos arquitecturas válidas, y cuándo elegir cada una:

| | Una base compartida con filtro de inquilino | Una base por inquilino o enjambre |
|---|---|---|
| Memoria | La más baja | +~12 MiB por base abierta (más sus vectores mapeados) |
| Aislamiento | Por filtro: un olvido del filtro mezcla datos | Por directorio |
| Búsqueda vectorial | Con filtro → camino exacto, **coste proporcional al tamaño del inquilino** (ver abajo) | Sin filtro → grafo HNSW: 1–2 ms con 100k vectores |
| Bases abiertas a la vez | 1 | Una por inquilino activo; ciérralas al terminar |

**Coste de la búsqueda vectorial filtrada** (vectores aleatorios de 384 dimensiones en una base de
20.000 documentos): 200 documentos por inquilino → 0,4 ms; 2.000 → 3,8 ms; 10.000 → 20 ms. Crece de forma
lineal con el tamaño del inquilino porque el motor calcula el coseno exacto sobre sus documentos (garantiza
los mejores `k`). Para memorias pequeñas (cientos o pocos miles de documentos por usuario) es inmediato; **si
un inquilino llega a decenas de miles, dale su propia base.** Mezclar ambos modelos es válido: los pequeños
comparten una base y los grandes tienen la suya.

Límites que conviene saber:
- Una base solo la abre un proceso a la vez: el servicio que atiende a los usuarios es el dueño de las bases.
- El motor es una librería para Bun/Node (`@johpaz/hive-db`): se integra con LangGraph.js, LangChain.js o
  hive-sdk. No hay binding para Python, así que LangChain/LangGraph en Python no pueden usarlo directamente.
- Todas las bases de una aplicación usan el mismo modelo; cambiar de modelo obliga a reindexar (ver §4.4).

---

## 6. Operación

| Tema | Qué hacer |
|---|---|
| **Copias de seguridad** | Con la base **cerrada**, copia el directorio entero. No hay todavía una API de instantánea consistente con la base abierta (*pendiente*). |
| **Dónde vive** | Un directorio fijo por instalación (`~/.hivecode/data/hivedb`, `~/.hive/data/hivedb`), nunca relativo al directorio de trabajo: si no, otro directorio es otra base vacía. |
| **Mantenimiento** | `compactIndex()` reescribe los vectores sin espacio muerto; el motor ya lo hace solo cuando sobra el 25 % y hay 1.024 o más muertos. |
| **Retención del log** | El log no borra eventos. Cada agente que escribe crea su propio fichero; decide una política (por ejemplo, un `agentId` por sesión y archivar las antiguas). |
| **Tras un fallo** | No hace falta hacer nada: al abrir se descartan las escrituras a medias y se reconstruyen los índices derivados. |
| **Memoria** | Los vectores van mapeados desde disco, así que el sistema puede liberarlos; `close()` libera el resto. |

Errores que verás y qué significan:

| Error | Causa | Qué hacer |
|---|---|---|
| `Database already open` | Otro proceso (o instancia) tiene la base | Un solo propietario (§3.1) |
| `VECTOR_SPACE_MISMATCH` | Abriste con otro modelo o dimensión distintos de los de la base | Usa el mismo `spaceId`/`embedder`, o reindexa en una base nueva |
| `EMBEDDER_UNAVAILABLE` | Binario compilado a mano sin la feature, modelo sin descargar y sin red, u `HIVEDB_OFFLINE=1` sin modelo | Precarga con `HiveDB.prepareEmbedder`, o ver «Instalaciones sin red» en la guía de uso |
| Error de dimensión o vector inválido | Dimensión distinta, NaN o vector nulo | Valida el vector antes de indexar |

---

## 7. Qué NO hacer

- **No uses el log como cola de trabajos.** Las suscripciones son *at-least-once*; un trabajo
  crítico necesita confirmaciones idempotentes en tu capa.
- **No dupliques el estado en el log «por si acaso».** El log es para hechos; para el estado
  actual usa colecciones.
- **No abras la base desde varios procesos** ni desde cada módulo: una instancia compartida.
- **No indexes en el camino de la respuesta si puedes hacerlo después.** Con embedder local, indexar
  cuesta ~20 ms por documento; hazlo en segundo plano.
- **No compares puntuaciones entre modos.** BM25, coseno y RRF están en escalas distintas.
- **No cambies de modelo de embeddings «en caliente».** Es una migración: base nueva y reindexado.

---

## 8. Plan de adopción por proyecto

Estado de partida: hive, hiveCode, hive-sdk y HiveLearn abren HiveDB **sin vectores** (solo
colecciones, BM25 y, en algunos, el log causal). Estas son las mejoras, de mayor a menor valor.

### hiveCode (el que más gana)

hiveCode ya tiene toda la estructura: selectores de herramientas, skills y playbook sobre BM25
(`tool-selector.ts`, `skill-selector.ts`, `playbook-selector.ts`, sobre `capability-search.ts`), un
compilador de contexto (`context-compiler.ts`), compactación de conversaciones, un reflector/curador
ACE y detección de bucles.

1. **Búsqueda de capacidades por significado.** Activar el embedder en `getHiveDb()` y dejar que
   `capability-search.ts` consulte en modo híbrido. «Mandar un correo» encontrará `send_email`
   aunque no comparta palabras, y «run the tests» encontrará la skill de pruebas. Recalibra los
   umbrales (§4.3): `applyRelativeCutoff` sobre RRF ya no es BM25.
2. **No reindexar el catálogo en cada arranque.** `replaceCapabilityDocs` borra y reinserta todo; con
   embeddings eso son segundos de CPU cada vez. Guarda un hash por documento y haz `upsert` solo de
   los cambios.
3. **Memoria de largo plazo de conversaciones.** `compaction.ts` ya genera resúmenes: además de
   guardarlos en la colección `summaries`, indexa cada resumen (`type=summary`, `thread=…`) y recupera
   los relevantes antes de cada turno en vez de arrastrar siempre el último.
4. **Episodios para el harness.** Al terminar una tarea, indexa un episodio (`kind=episode`:
   objetivo, qué se hizo, cómo acabó). Así `buildAgentContext` con `episodicSimilarity` trae casos
   parecidos, y el reflector/curador ACE dispone de ejemplos reales.
5. **Contexto de tareas largas con `buildAgentContext`** en lugar de recortar a mano, y
   `causalThread` + `evaluateHarness` como señal adicional del detector de bucles
   (`stuck-loop.ts`): `ErrorLoop` y `ObjectiveDrift` salen del log sin lógica propia.
6. **Memoria del proyecto.** Indexa hechos del repositorio que el agente descubre (convenciones,
   comandos, decisiones de arquitectura) con `project=<ruta>`, y recupéralos al abrir ese proyecto.

### hive-sdk, hive y HiveLearn

- Activar el embedder en la búsqueda de capacidades (como hiveCode).
- hive-sdk: ya separa el catálogo compartido de la elección por inquilino; mantén ese patrón y añade
  el hash por documento (regla 7).
- HiveLearn: indexar el material de cada lección y las respuestas del alumno permite recuperar por
  significado; mantén el filtro de curso/alumno en todas las consultas.

### Hive Cloud

- Una base con ámbito por filtro (§5.6) es correcta; asegura que **toda** consulta del índice lleve
  `tenant`, y revisa la retención del log causal antes de activarlo en producción.
- Decide el modelo de embeddings **ahora**: cambiarlo después obliga a reindexar todos los
  inquilinos. Para un servicio multiusuario, plantéate si el embedding lo hace el servidor (local; es un único modelo compartido por todos los usuarios, ver §5.7)
  o una API; en ambos casos el `spaceId` debe ser único y estable.

---

## 9. Antes de publicar la versión nueva

Lista de comprobación, en orden:

1. **Usar la 0.6.1 o posterior.** Los paquetes publicados ya traen el embedder (la 0.6.1 corrige la
   fusión híbrida: orden determinista, más candidatos que `k` y palabras vacías fuera). Verifica el modelo
   en tu entorno con `HiveDB.prepareEmbedder()` antes de abrir la base.
2. **Subir la dependencia en cada repo** (el formato en disco cambió en la 0.6: los vectores van en un
   fichero plano). Los proyectos dependen de `^0.5.1`, que en 0.x **no** admite 0.6.x: actualiza cada
   repo explícitamente.
3. **Revisar los umbrales de relevancia:** con la búsqueda híbrida las puntuaciones son RRF; calibra con
   `vectorScore`/`textScore` (ver §4.3).
4. **Actualizar cada repo y pasar sus pruebas:** hiveCode primero. Prueba además con una copia de
   una base real (las bases del formato anterior se migran solas al abrir, sin vuelta atrás).
5. **Decidir** la política de retención del log y quién es el propietario de la base en cada
   proceso.
