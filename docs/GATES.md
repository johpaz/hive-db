Cada gate es un hito de la construcción del motor y tiene su archivo de tests `gN_*.rs` (y, si
aplica, `gN_*.test.ts`). Este documento conserva el historial; para ver qué hace HiveDB hoy,
empieza por el [README](../README.md).

## Gates completados

- ✅ Fase 0: workspace Rust y CI mínima.
- ✅ G1: Event Log append-only sobre `redb`, `seq` monotónico asignado por el motor.
- ✅ G2: Proyecciones deterministas (`CurrentFacts`, `TaskState`) con replay idéntico.
- ✅ G3: Working memory con TTL (`DashMap`).
- ✅ G4: Semantic memory híbrida (`tantivy` + HNSW propio + RRF).
- ✅ G5: Reactive engine con suscripciones push.
- ✅ G6: Consent Graph (`can()`, `IntentLogged`, expiración controlada).
- ✅ G7: Concurrencia particionada por `agent_id` + test `loom`.
- ✅ G8: napi-rs binding + capa TypeScript (`@johpaz/hive-db`).
- ✅ G9: Harness de larga duración (`CausalThread`, `buildAgentContext`, `HarnessLoop`): memoria causal de tareas, ventanas de contexto adaptativas y evaluación de proceso.
- ✅ G10: Distribución multiplataforma con `@napi-rs/cli` (6 targets: linux x64 gnu/musl, linux arm64, macOS x64/arm64, Windows x64).
- ✅ G11: Colecciones de documentos (CRUD mutable sobre `redb`): versionado optimista, índices secundarios de igualdad (con `unique`), scan con prefijo/orden/limit y batches atómicos multi-colección.
- ✅ G11b: Integridad semántica: documentos autoritativos en `redb`, espacio vectorial explícito, validación estricta, filtros exactos y compactación HNSW.

> Nota sobre numeración: los tests de colecciones se llaman `g9_collections.rs`/`g9_collections.test.ts` por una colisión histórica con la numeración del README; el gate funcional de colecciones es G11.
- ✅ Embedder local opcional (`hivedb-embed`, `multilingual-e5-small` sobre `candle`).
- ✅ HNSW propio de almacenamiento plano y una sola copia de los vectores (ver `docs/BENCHMARKS.md`).
