# @johpaz/hive-db

Motor de base de datos embebido, local-first y agent-native para agentes de IA: event-log inmutable, proyecciones deterministas, búsqueda híbrida BM25 + vectorial, colecciones de documentos mutables, grafo de consentimiento y suscripciones reactivas — todo en un solo directorio, sin daemon ni dependencias de red.

Núcleo en Rust (`redb` + `tantivy` + HNSW propio), expuesto a Bun/Node vía `napi-rs`. ¿Usas Python (LangChain, LangGraph)? El mismo motor está en PyPI como [`johpaz-hive-db`](../hive-db-py/README.md), con adaptadores en [`johpaz-langchain-hivedb`](../langchain-hivedb/README.md).

## Instalación

```bash
bun add @johpaz/hive-db
# o: npm install @johpaz/hive-db / pnpm add @johpaz/hive-db
```

Incluye binarios precompilados para Linux x64 (glibc y musl), Linux arm64, macOS x64/arm64 y Windows x64 — no necesitas Rust instalado.

### Embeddings sin configurar nada (opcional)

Los binarios incluyen un embedder local (`multilingual-e5-small`, español e inglés, en CPU): con
`embedder: "local"` HiveDB genera los vectores a partir del texto. Los pesos del modelo (~470 MB) **no**
vienen en el paquete: se descargan la primera vez que lo activas, con reintentos y reanudación. Para
mostrar el avance en lugar de esperar en silencio:

```ts
import { HiveDB } from "@johpaz/hive-db";

await HiveDB.prepareEmbedder({
  onProgress: ({ file, downloaded, total }) => console.log(file, Math.floor((100 * downloaded) / total) + "%"),
});
const db = await HiveDB.open("./data", { embedder: "local" });
await db.upsertDoc({ id: "a", body: "Cómo configurar tu cuenta de email" });
await db.queryHybrid({ text: "correo electrónico", k: 3 }); // encuentra "a" aunque no comparta palabras
```

`HIVEDB_OFFLINE=1` impide cualquier acceso a la red; `HIVEDB_MODEL_DIR` cambia dónde se guarda el modelo y
`HIVEDB_MODEL_BASE_URL` apunta a un espejo. También puedes aportar tus propios vectores (de cualquier modelo
o API) con `vector: { dimension, spaceId }`. Ver la
[guía de uso](https://github.com/johpaz/hive-db/blob/main/docs/USER_GUIDE.md).

## Uso rápido

```ts
import { HiveDB } from "@johpaz/hive-db";

const db = await HiveDB.open("./data/my-agent", {
  vector: { dimension: 768, spaceId: "my-model:768:retrieval-v1" },
});

// Event-log: única vía de escritura, seq asignado por el motor
const seq = await db.append({
  agentId: "travel-agent-7",
  streamId: "trip-to-paris",
  kind: "Fact",
  payload: JSON.stringify({ temperature: 21.5 }),
});

// Búsqueda híbrida: BM25 (con stemming en español) + vectorial + RRF
await db.upsertDoc({ id: "doc-1", body: "genera reportes de transacciones" });
const hits = await db.queryHybrid({ text: "transaccion", k: 5 }); // matchea "transacción"

// Colecciones: CRUD mutable con versionado optimista e índices secundarios
const agents = db.collection<{ name: string; role: string }>("agents");
await agents.put("a1", { name: "Atlas", role: "worker" });
await agents.createIndex("role");
const workers = await agents.findBy("role", "worker");

db.close();
```

## Qué incluye

| Capa | Motor |
|---|---|
| Event log append-only + proyecciones | `redb` |
| Búsqueda de texto (BM25, stemming español, sin palabras vacías en es/en) | `tantivy` |
| Embeddings locales (opcional) | `candle` |
| Búsqueda vectorial (ANN) | HNSW propio |
| Fusión de resultados híbridos | Reciprocal Rank Fusion propio |
| Colecciones de documentos (CRUD mutable) | `redb` |
| Grafo de consentimiento / intent audit | proyección sobre el event log |
| Suscripciones reactivas | push, no polling |

## Rendimiento

Con 100.000 frases reales (384 dimensiones, disco NVMe): búsqueda vectorial ~1,4 ms (p50) con
recall@10 ≈ 0,98, búsqueda híbrida ~2,8 ms, apertura de una base poblada ~40 ms, inserción por lotes
~5.200 documentos/s, 241 MiB en disco y ~31 MiB de memoria anónima. Cada consulta puede ajustar
precisión y velocidad con `efSearch` (por defecto 200). Metodología, comparación con sqlite-vec,
LanceDB y libSQL y comandos para reproducirlo: [`docs/BENCHMARKS.md`](https://github.com/johpaz/hive-db/blob/main/docs/BENCHMARKS.md).
Historial de cambios: [`CHANGELOG.md`](https://github.com/johpaz/hive-db/blob/main/CHANGELOG.md).

## Documentación completa

- [`docs/USER_GUIDE.md`](https://github.com/johpaz/hive-db/blob/main/docs/USER_GUIDE.md) — guía de uso desde Bun/TypeScript, con ejemplos de cada API.
- [`docs/IMPLEMENTATION.md`](https://github.com/johpaz/hive-db/blob/main/docs/IMPLEMENTATION.md) — manual de implementación y extensión del motor.
- [`docs/DISTRIBUTION.md`](https://github.com/johpaz/hive-db/blob/main/docs/DISTRIBUTION.md) — cómo se distribuyen los binarios multiplataforma.

## Principios de diseño

1. Cero dependencia de servicios externos (soberanía digital).
2. El event-log es la fuente de verdad: todo estado es una proyección derivada.
3. Corre embebido, in-process, dentro de la aplicación consumidora.
4. Los primitivos del agente (consentimiento, memoria, reactividad) viven en el motor, no se simulan por encima.

## Licencia

Apache-2.0
