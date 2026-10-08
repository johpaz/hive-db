# HiveDB — Manual de implementación

> Guía para desarrolladores que mantienen o extienden el motor HiveDB.

---

## 1. Estructura del workspace

```
hive-db/
├── Cargo.toml                 # workspace Rust
├── package.json               # workspace Bun
├── README.md
├── SPEC.md                    # especificación de diseño
├── TDD.md                     # contratos de test por fase
├── crates/
│   ├── hivedb-core/           # motor: log, proyecciones, consent, reactivo
│   ├── hivedb-index/          # índice semántico híbrido
│   └── hivedb-napi/           # binding napi-rs (cdylib)
└── packages/
    └── hive-db/               # envoltorio TypeScript para Bun
```

| Crate / Paquete | Responsabilidad | Tecnologías clave |
|---|---|---|
| `hivedb-core` | Event-log sharded, proyecciones deterministas, working memory, motor reactivo, consent graph | `redb`, `dashmap`, `tokio`, `serde_json` |
| `hivedb-index` | BM25 full-text (`tantivy`), ANN vectorial (HNSW propio), fusión RRF | `tantivy`, `rayon`, `memmap2` |
| `hivedb-napi` | Expone `HiveDB` al runtime JS vía napi-rs | `napi`, `napi-derive`, `tokio` |
| `@johpaz/hive-db` | API ergonómica TypeScript, async iterators, tipos | Bun |

---

## 2. Filosofía de implementación

1. **El log es la fuente de verdad.** Todo estado es una proyección derivada.
2. **Append-only.** Nunca se muta un evento existente.
3. **Cliente no controla `seq` ni `timestamp`.** El motor los asigna para garantizar orden y auditoría.
4. **Particionamiento por `agent_id`.** Cada agente tiene su propio shard `redb`; escrituras de distintos agentes no se bloquean. La persistencia de `next_seq` se hace dentro del shard global solo cuando un evento global ya lo toca (`ConsentGranted`, `ConsentRevoked`, `IntentLogged`) o en el `Drop` de `HiveDB`, para no introducir contención cruzada en cada append.
5. **Determinismo puro.** Una proyección debe producir el mismo estado reproduciendo el log.

---

## 3. El event log (`hivedb-core/src/log.rs`, `shard.rs`)

### Shards

- Un directorio base contiene:
  - `shards/<agent_id>.redb` — un shard por agente.
  - `shards/_global.redb` — shard global para proyecciones y registro de consentimiento.
- `next_seq` y `seq_to_agent` viven en memoria mientras la base está abierta. `next_seq` se persiste en la tabla `meta` de `_global.redb` bajo `"next_seq"`; `seq_to_agent` se reconstruye escaneando shards al abrir.

### Flujo de `append`

1. `HiveDB::append(input)` → `LogHandle::append`.
2. Se asigna `seq = next_seq + 1`.
3. Se resuelve el shard del `agent_id` (o se crea).
4. En una transacción `redb`:
   - Guarda el evento.
   - Aplica handlers de proyección de agente.
   - Si el evento afecta proyecciones globales, también aplica handlers en `_global.redb` y, en la misma sección crítica, persiste `next_seq` en la tabla `meta`.
5. Se dispara el evento por el `ReactiveEngine`.
6. Al cerrar la base (`Drop` de `HiveDB`), se hace un flush de `next_seq`.

### Tests de log

Los tests de G1 viven en `crates/hivedb-core/tests/g1_log.rs` y verifican monotonía global, inmutabilidad, ausencia de API de mutación, y persistencia de `next_seq` tras reapertura (`next_seq_survives_reopen_without_rescan`) y fallback al escaneo cuando no hay meta (`reopen_without_meta_falls_back_to_scan`).

---

## 4. Proyecciones (`hivedb-core/src/projection.rs`)

Una proyección es un *fold* determinista sobre el log.

```rust
pub trait Projection: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + PartialEq + Debug + Default + Clone;
    fn name() -> &'static str;
    fn scope() -> ProjectionScope { ProjectionScope::Agent }
    fn apply(state: &mut Self::State, event: &Event);
    fn merge(whole: &mut Self::State, part: &Self::State) { *whole = part.clone(); }
}
```

### Cómo añadir una proyección

1. Define el tipo de estado en `hivedb-core/src/state/`.
2. Implementa `Projection`.
3. Regístrala en `default_registry()` en `db.rs`:

```rust
fn default_registry() -> ProjectionRegistry {
    let mut registry = ProjectionRegistry::empty();
    registry.register::<CurrentFacts>();
    registry.register::<TaskState>();
    registry.register::<ConsentGraph>();
    registry.register::<ToolLedger>();
    registry.register::<MiProyeccion>(); // <-- nuevo
    registry
}
```

4. Añade test de G2/G3 que verifique replay determinista y checkpoints.

### Scope

- `Agent`: se mantiene una copia del estado por shard de agente. Útil para hechos vigentes, tareas, métricas de herramientas, etc.
- `Global`: un único estado en `_global.redb`. Útil para consentimiento.

### Merge

Para proyecciones de agente cuyo estado sea un mapa, sobrescribe `merge` para combinar resultados parciales de cada shard en lugar de reemplazarlos.

---

## 5. Eventos (`hivedb-core/src/event.rs`)

### Añadir un nuevo `EventKind`

1. Añade la variante en `EventKind`.
2. Añade el tag correspondiente en `EventKindTag`.
3. Implementa la conversión `From<&EventKind> for EventKindTag`.
4. Si el evento requiere payload estructurado, define la serialización en el JSON del `payload`.
5. Actualiza `js_to_event_input` en `crates/hivedb-napi/src/lib.rs` para traducir desde JS si es necesario.
6. Actualiza el tipo `EventInput.kind` en `packages/hive-db/src/index.ts`.

### Regla de oro

No expongas `seq` ni `timestamp` en `EventInput`. Hay un test `compile_fail` (`tests/ui/event_input_no_seq.rs`) que lo garantiza.

---

## 6. Motor reactivo (`hivedb-core/src/reactive.rs`)

- `ReactiveEngine` mantiene un `DashMap<u64, (EventPattern, UnboundedSender<Event>)>`.
- `subscribe(pattern)` devuelve una `Subscription` con receptor `tokio::mpsc::unbounded`.
- `dispatch(event)` itera suscriptores y envía clones si el evento coincide con el patrón.
- `EventPattern` puede incluir un `Predicate` sobre el payload: `Always`, `Eq { path, value }` (JSON pointer) o `Contains { path, value }` (array/string).
- Semántica **at-least-once**: si el receptor se cerró, el `send` falla silenciosamente.

### En el binding napi

`JsHiveDB::subscribe` lanza una tarea `tokio` que lee del `Subscription` y llama a `ThreadsafeFunction` en modo no bloqueante. La callback JS recibe `(err, event)`.

---

## 7. Índice semántico (`hivedb-index`)

### Componentes

| Módulo | Responsabilidad |
|---|---|
| `text.rs` | `TextIndex` sobre `tantivy`: indexado y BM25. |
| `hnsw.rs` | `VectorIndex`: ids de documento, tombstones y volcado a disco sobre el grafo plano. |
| `flat_hnsw.rs` | HNSW propio de almacenamiento plano (`u32` + vectores mapeados con `mmap`), construcción por tandas en paralelo y determinista. |
| `vector_file.rs` | Fichero plano de vectores normalizados (copia autoritativa) y su vista mapeada en memoria. |
| `embed.rs` | Trait `Embedder`: lo implementa `hivedb-embed` (`multilingual-e5-small` sobre `candle`). |
| `rrf.rs` | Fusión Reciprocal Rank. |
| `index.rs` | `SemanticIndex` orquesta text + vector + RRF. |

### Espacio vectorial

La búsqueda vectorial se activa explícitamente mediante `OpenOptions { vector: Some(VectorOptions { dimension, space_id }) }`. Sin configuración, la base funciona en modo BM25 y rechaza vectores. `meta.json` persiste versión de esquema, dimensión, `space_id` y métrica coseno; cualquier diferencia al reabrir falla con `VECTOR_SPACE_MISMATCH`.

#### Bases creadas por 0.3.x

El `meta.json` de 0.3.x sólo tenía `{"vector_dimension": N}`. Al abrir una base así, `resolve_database_meta()` la migra al esquema 2 con la configuración vectorial pedida en esa apertura y **conserva `vector_dimension`**, la única clave que lee 0.3.x: la base se puede volver a abrir con 0.3.x si hay que dar marcha atrás. La reescritura va por archivo temporal + rename. Un `meta.json` que ya tiene `schema_version` se compara y no se reescribe nunca.

0.3.x no guardaba los documentos del índice en un almacén autoritativo (Tantivy sólo conserva `id` y `filters`), así que no hay nada que migrar: tras la primera apertura el índice semántico está vacío y el consumidor debe reindexar sus documentos. Colecciones y event-log no dependen de `meta.json` y quedan intactos; `vec/` se deja en disco sin usar. El fixture `crates/hivedb-index/tests/fixtures/v0.3.1`, generado con 0.3.1, cubre este camino.

### Persistencia, reconstrucción y compactación

Qué es autoritativo y qué es derivado:

| Fichero | Contenido | Naturaleza |
|---|---|---|
| `semantic.redb` | cada `IndexDoc` **sin vector** más la ranura (`slot`) de su vector; generación; ranuras confirmadas; cuál de los dos ficheros de vectores está activo | autoritativo |
| `vectors.0.dat` / `vectors.1.dat` | los vectores, normalizados (L2), en `f32` little-endian, contiguos; cabecera de 16 bytes y una fila por ranura | autoritativo (solo uno está activo) |
| `fts/`, `fts.generation` | índice BM25 de Tantivy y la generación con la que se cerró limpiamente | derivado |
| `hnsw/` (`vectors.hnsw.graph`, `vectors.meta`) | grafo HNSW y el mapa ranura → id | derivado |

Los vectores se guardan **una sola vez**. El grafo no los copia: lee del mismo fichero mapeado en
memoria, y el id de cada nodo es su número de ranura. Se normalizan al escribirlos (la métrica es
coseno), por lo que no se conserva la magnitud original; ninguna API devuelve el vector guardado.

**Escritura de una tanda:** (1) las filas nuevas se escriben al final del fichero y se hace `fsync`,
(2) se confirma la transacción de `redb` que referencia las ranuras y sube la generación, (3) se
publica la nueva vista de lectura y se enlazan los nodos. Si el proceso muere entre (1) y (2) quedan
filas huérfanas: al abrir se descartan recortando el fichero a las ranuras confirmadas en `redb`.
Que el fichero tenga menos filas de las confirmadas es corrupción y la apertura falla con un error.

**Actualizar o borrar un documento** deja su ranura muerta (un tombstone en el grafo). Con al menos
1.024 ranuras muertas que sean el 25% o más, o al llamar a `compact_index()` / `compactIndex()`, se
reescribe el vector en el fichero **alterno** (sin ranuras muertas) y una sola transacción de `redb`
renumera las ranuras y cambia el fichero activo; el grafo se reconstruye. Sin `rename` sobre ficheros
mapeados, y un fallo antes de confirmar deja el fichero activo intacto (el otro se borra al abrir).

**Apertura:** si el cierre anterior dejó el índice de texto y el grafo al día con la generación, no se
lee ningún documento (arranque de decenas de milisegundos con 100k vectores). Si algo no cuadra
—marcador ausente, generación distinta, recuento de documentos o de vectores vivos— se reconstruye lo
derivado desde `semantic.redb` y los vectores. Las mutaciones están serializadas por un `RwLock` de
operación y aumentan una generación dentro de la misma transacción `redb`.

#### Migración desde el formato anterior

Hasta la versión anterior el vector iba serializado dentro del registro del documento en la tabla
`semantic_docs`. Al abrir una base así, `migrate_legacy_store` escribe un fichero `redb` **nuevo**
(`semantic.redb.migrating`) con los documentos sin vector en `semantic_docs_v2` (por tandas de 2.000) y
los vectores normalizados en el fichero plano, y al terminar lo sustituye por el antiguo con un
`rename`. El original no se toca hasta ese momento: una interrupción en cualquier punto lo deja intacto
y la siguiente apertura repite la migración (un fichero de vectores de un intento anterior se descarta).
Se hace así porque copiar a otra tabla del mismo fichero y compactar lo dejaba más grande que antes (la
compactación de `redb` no reclama bien los valores grandes). `meta.json` **no se modifica** (el esquema
sigue siendo 2).

Para que una versión anterior no pueda abrir la base migrada y ver un índice vacío, `semantic_docs`
se recrea vacía con claves `u64`: abrirla con claves `&str` falla con un error de tipo. No hay vuelta
atrás automática; para volver a una versión anterior hay que reindexar.

### HNSW propio (`flat_hnsw.rs`)

Sustituye a la crate `hnsw_rs`, que dominaba el arranque (~0,9 s con 100k vectores, un objeto por
vecino al cargar), duplicaba los vectores y obligaba a un `Box::leak` y un `catch_unwind` porque
hacía `panic!` con ficheros corruptos.

- **Estructura:** `M = 24` vecinos por nodo y capa (el doble en la capa 0), `ef_construction = 100`,
  nivel geométrico con parámetro `1/ln(M)` derivado del id del nodo (reproducible). El grafo son
  arrays `u32` (listas de vecinos de capacidad fija y su longitud en `u8`); las capas superiores solo
  guardan los nodos que las alcanzan. El id de un nodo es su ranura en el fichero de vectores.
- **Búsqueda:** descenso voraz por las capas altas y búsqueda con cola de prioridad (`ef`) en la capa 0.
  Conjunto de visitados en un bitset; las distancias, enteros ordenables (los `f32` no negativos
  conservan su orden por bits). Sólo lee: varias consultas corren a la vez sin bloquearse.
- **Distancia:** `1 − a·b` sobre vectores normalizados, con un producto punto en `f32` y 16
  acumuladores independientes que el compilador vectoriza (SSE/AVX/NEON) sin `unsafe`. La
  `DistCosine` de `anndists` usaba tres acumuladores `f64` por par y no se vectorizaba.
- **Poda:** heurística de diversidad del paper original (un candidato entra si está más cerca del nodo
  que de cualquiera de los ya elegidos), rellenando con los descartados hasta la capacidad.
- **Construcción en paralelo, sin locks y determinista:** los vectores nuevos se enlazan por tandas
  de ≈ 1/32 de lo ya enlazado (máx. 4.096). En cada tanda se calculan en paralelo (sólo lectura) los
  vecinos de cada nodo nuevo; después se aplican las aristas directas y las inversas, agrupadas por
  destino y ordenadas. Ningún hilo escribe mientras otros leen, así que el resultado no depende del
  número de hilos: la misma entrada da exactamente el mismo grafo (hay un test).
- **Persistencia:** `hnsw/vectors.hnsw.graph` (sólo el grafo) y `hnsw/vectors.meta` (generación,
  `space_id`, dimensión, mapa ranura → id y tombstones), que se escribe el último por `rename`. Al
  cargar se valida todo (longitudes, rangos, que cada vecino llegue a la capa en la que aparece, que
  el nivel máximo coincida y que el número de nodos coincida con el de ranuras del fichero de
  vectores); ante cualquier duda se reconstruye. Los volcados con vectores de versiones anteriores
  se ignoran y su fichero `vectors.hnsw.data` se borra.
- **Parámetros y su error histórico:** `Hnsw::new` de `hnsw_rs` recibe `(M, max_elements, max_layer,
  ef_construction)`; los argumentos iban en otro orden y el índice se construía con
  `ef_construction = 16`, lo que dejaba el recall en 0,43 con `ef = 200`. Con el motor propio los
  parámetros son constantes con nombre en `hnsw.rs`.

### Rendimiento

El benchmark reproducible vive en `crates/hivedb-bench` (los comparadores de Python, en
`comparadores/`). Con 100.000 frases reales de 384 dimensiones, `ef = 200` y disco NVMe: vector p50
1,4 ms (p99 ~2 ms), recall@10 0,982, texto 2,5 ms, híbrido 4,3 ms, ingesta ~4.900 docs/s, apertura
~46 ms, cierre ~42 ms, 242,5 MiB en disco y ~31 MiB de memoria anónima. Las curvas de `ef`, la
comparación con sqlite-vec, LanceDB y libSQL y los comandos exactos están en
[`BENCHMARKS.md`](BENCHMARKS.md). Si cambias el motor vectorial, vuelve a medir (en un directorio en
disco real: `/tmp` suele ser `tmpfs`).

### Filtros escalares

Actualmente solo `ScalarFilter::Eq { field, value }`, sobre **cualquier campo**. Cada filtro se indexa como token `campo\u{1F}valor` en un campo `STRING` multivalor de `tantivy`. En consultas vectoriales, Tantivy obtiene la allowlist y el motor calcula coseno exacto sobre esos vectores, garantizando los mejores `min(k, permitidos)` sin post-filter de recall limitado.

### Añadir una nueva estrategia de fusión

1. Añade variante en `Fusion`.
2. Implementa la lógica en `SemanticIndex::query_hybrid`.
3. Expón el parámetro en TS si aplica.

---

## 8. Colecciones de documentos (`hivedb-core/src/collections.rs`)

CRUD mutable sobre `redb`, independiente del event-log — la contraparte de tablas relacionales de SQLite. Añadido en el gate G10 (el nombre de archivo de tests quedó como `g9_collections.rs`/`g9_collections.test.ts` por una colisión de numeración con el gate G9 de distribución napi; es cosmético).

### Tablas `redb`

| Tabla | Clave | Contenido |
|---|---|---|
| `col_docs` | `(collection, id)` | `StoredDoc { version, json }` |
| `col_index_entries` | `(collection, field, value_token, id)` | entrada de índice secundario (marcador, sin valor) |
| `col_index_defs` | `(collection, field)` | `IndexDef { unique }` — persiste entre reaperturas |

### Versionado optimista

Cada `put` incrementa `version`. `PutOptions.expected_version`: `None` = sin chequeo, `Some(0)` = crear-solo, `Some(n)` = debe coincidir con la versión actual o falla con `version conflict`.

### Índices secundarios

`col_create_index(collection, field, unique)` recorre (`scan`) todos los docs existentes y los indexa (backfill). Si `unique` y hay un duplicado, la creación falla y no deja índice a medias — se revisa el conjunto completo antes de escribir cualquier entrada. Solo valores JSON escalares (string/number/bool) se indexan; arrays, objetos y campos ausentes se omiten sin error. El mantenimiento del índice (altas/bajas/cambios de valor) ocurre dentro de la misma transacción que el `put`/`delete` del documento vía los helpers compartidos `put_in_txn()` / `delete_in_txn()`.

### Batches atómicos

`col_batch(&[ColOp::Put{..} | ColOp::Delete{..}])` abre una única transacción `redb` (`begin_write()`) y aplica cada op con los mismos helpers `put_in_txn`/`delete_in_txn` que usan los métodos de una sola operación — garantiza que un fallo a mitad de la lista (p. ej. version conflict) aborta toda la transacción sin dejar cambios parciales.

### Añadir un nuevo método de colección

1. Implementa la función en `collections.rs` operando sobre `&Database` o dentro de una `WriteTransaction` si necesita atomicidad con otras ops.
2. Expón el facade en `db.rs` (`col_*`).
3. Añade el método napi en `hivedb-napi/src/lib.rs` (tipos `Js*` si el payload cruza la frontera FFI).
4. Añade el método en la clase `Collection<T>` de `packages/hive-db/src/index.ts`.

---

## 9. Harness de larga duración (`hivedb-core/src/causal/`, `context.rs`, `harness.rs`)

El harness convierte el event-log en memoria causal para agentes de larga duración. Tiene tres piezas:

1. **`CausalThread`** (`causal/mod.rs`). Reconstruye, para un `stream_id`, el grafo de decisiones y llamadas a herramientas siguiendo `causation`. Detecta dos anomalías:
   - `ErrorLoop`: misma herramienta con el mismo error ≥ 3 veces.
   - `ObjectiveDrift`: decisiones cuya `correlation` difiere del `IntentLogged` inicial.
2. **`build_agent_context`** (`context.rs`). Construye una ventana de contexto acotada en tokens. Estrategias:
   - `causal_anchors`: camina hacia atrás desde el objetivo actual incluyendo decisiones causales lejanas.
   - `compress_completed_phases`: resume fases terminadas en `PhaseSummary` + decisiones clave; mantiene la fase actual completa.
   - `episodic_similarity`: consulta el índice híbrido filtrando por `kind=episode` y mezcla vector + texto.
   - `recent_anomalies`: incluye siempre las anomalías detectadas.
3. **`HarnessLoop::evaluate`** (`harness.rs`). Función pura que recibe un `CausalThread` (y episodios similares) y produce:
   - `process_quality` (penalizado por anomalías),
   - `output_quality` (derivado del estado final),
   - `root_cause` (decisión más temprana con más descendencia de fallos),
   - `findings` (`InefficientLoop`, `ObjectiveDrift`, `RootCause`, `InsufficientEvidence`),
   - `proposals` (`LearningProposal`) con `evidence_seqs`, `trigger_seq`, `confidence` y `specificity`.

El evaluador no tiene side effects; el llamador persiste las proposals como eventos `LearningProposal` si quiere auditarlas.

### Contrato de wire (napi/JSON)

`causalThread()`, `buildAgentContext()` y `evaluateHarness()` devuelven JSON serializado directamente desde los structs de Rust (`serde_json::to_string`, sin capa de mapeo intermedia como el resto de `#[napi(object)]`). Todos los structs/enums públicos de `causal/mod.rs`, `context.rs` y `harness.rs` llevan `#[serde(rename_all = "camelCase")]`, así que el JSON expuesto a TS es camelCase (`toolCalls`, `processQuality`, `causedBy`, etc.) aunque los campos internos en Rust sigan snake_case. `ContextItem` usa tagging interno (`#[serde(tag = "type", ...)]`): cada item trae `type: "decision" | "toolCall" | "anomaly" | "episode" | "phaseSummary"` en vez del tagging externo por defecto de serde. La única excepción deliberada es `ToolOutcome` (`"Ok"` / `"Timeout"` / `{"Err": "..."}`), que es el contrato de **entrada** del payload `ToolCall.outcome` (ver `SPEC.md` §2.2 y `docs/AGENT_INTEGRATION.md`) y no se re-castea a camelCase.

### Límite de escalabilidad conocido

`HiveDB::causal_thread()` (`db.rs`) reconstruye el hilo bajo demanda leyendo **todo** el event log de todos los shards (`read_stream_all_agents`) y filtrando por `stream_id` — no lee desde una proyección checkpointeada. Existe un marcador de proyección `CausalThreadProjection` (`state/causal_thread.rs`) con `merge`/`apply` ya implementados, pero **no está registrado** en `default_registry()`, así que nunca participa del checkpointing incremental. En la práctica esto es O(eventos totales de todos los shards) por llamada, no O(eventos del stream), algo que se nota en tareas muy largas con muchos agentes activos (el test `g9d_e2e.rs` ya ejercita un caso de 650 eventos/3 sesiones). Registrar la proyección y leer desde ahí es trabajo de rendimiento pendiente, fuera del alcance del harness de contrato actual.

### Extensión

- Para añadir una nueva anomalía, modifica `causal/mod.rs::detect_anomalies` y el enum `AnomalyKind`.
- Para cambiar la política de tokens, ajusta `context.rs::estimate_tokens` y el recorte final por presupuesto.

---

## 10. Grafo de consentimiento (`hivedb-core/src/state/consent_graph.rs`)

- Proyección **global**.
- Estado: `BTreeMap<u64, Grant>` donde la clave es el `seq` del evento `ConsentGranted`.
- `find_active_grant(agent, action, resource, now)` busca un grant vigente (no revocado, no expirado, scope matching). La búsqueda es transitiva: una cadena `PM → Lead → Backend` autoriza a `Backend` mientras todos los eslabones sean vigentes. Se detectan ciclos con un `HashSet` de agentes visitados.
- `ConsentGraph::apply` reacciona a `ConsentGranted` (inserta) y `ConsentRevoked` (elimina por `grant_seq`).

### Auditoría

`HiveDB::can` siempre hace `append(IntentLogged { actor, intent, authorized_by })` y devuelve `Decision { allowed, intent_log_seq }`.

---

## 11. Binding napi-rs (`hivedb-napi`)

### Reglas importantes

- `u64` **no** es un tipo nativo JS en napi-rs 2.x. Expón `i64` y castea internamente.
- `ThreadsafeFunction` se escribe con `f` minúscula: `ThreadsafeFunction`.
- Los structs `#[napi(object)]` deben tener campos que implementen `FromNapiValue`/`ToNapiValue`.
- `Float32Array` se pasa directamente para vectores.
- La callback de `ThreadsafeFunction` recibe `(err, value)` en JS.

### Cómo exponer un nuevo método

1. Añade método en `#[napi] impl JsHiveDB`.
2. Convierte tipos JS ↔ Rust en funciones helper (`js_to_*`, `*_to_js`).
   - `u64` no es nativo de JS; usa `i64` en `#[napi(object)]` y castea.
3. Recompila con `cargo build -p hivedb-napi --release`.
4. Actualiza `packages/hive-db/src/index.ts` con tipos y wrapper.
5. Añade test en `packages/hive-db/test/`.

### Métodos expuestos recientemente

- `workingSet` / `workingGet` / `workingKeys`: memoria de trabajo efímera con TTL.
- `subscribe` / `events` con `Predicate` (`Eq`, `Contains`, `Always`).
- `toolStats(tool)`: métricas agregadas de `ToolCall` desde la proyección `ToolLedger`.
- `lastSeq()`: último `seq` asignado.

### Construcción del `.node`

```bash
cd packages/hive-db
bun run build:napi   # cargo build --release + cp libhivedb_napi.so -> hivedb-napi.node
```

El artefacto es un shared object renombrado a `.node` para que Bun/Node lo carguen.

---

## 12. Tests

### Rust

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Bun

```bash
cd packages/hive-db
bun run build:napi
bun test
```

### Convenciones

- Cada gate G1-G8 tiene su archivo de test (`g1_log.rs`, `g2_projections.rs`, `g2_tool_ledger.rs`, `g3_working.rs`, …).
- Los tests de propiedad usan `proptest` donde aplica.
- Los tests de concurrencia usan `loom` en `g7_concurrency.rs`.
- Los tests de compilación fallida usan `trybuild`.
- Los tests TS llevan ID `§4.11`, `§4.11b`, etc. del TDD.

---

## 13. Convenciones de código

- Formato con `rustfmt`.
- Clippy sin warnings (`-D warnings`).
- Errores propios: `HiveError` / `IndexError` con `thiserror`.
- `unsafe` solo en fronteras FFI/mmap; no en lógica de negocio.
- Nombres de proyecciones en PascalCase; nombres de test `snake_case` descriptivos.

---

## 14. Roadmap técnico próximo

1. Persistir `seq_to_agent` en `_global.redb` para evitar escanear todos los shards al abrir.
2. Añadir persistencia de checkpoints de proyección más granular.
3. Mejorar manejo de errores de callback en `ThreadsafeFunction`.

---

## 15. Distribución con `@napi-rs/cli`

El binding nativo se construye, empaqueta y publica usando `@napi-rs/cli` 3.x. Ver `docs/DISTRIBUTION.md` para la guía completa.

### Crates y versiones

- `napi` 3.10.1 + `napi-derive` 3.5.9 + `napi-build` 2.x en `crates/hivedb-napi/Cargo.toml`.
- `@napi-rs/cli` 3.7.2 en `packages/hive-db/package.json` (`devDependencies`).
- El crate `hivedb-napi` requiere el feature `tokio_rt` de `napi` (adicional a `napi8` y `async`) porque `subscribe` usa `tokio::spawn`])

### Configuración `napi` en `package.json`

```jsonc
"napi": {
  "binaryName": "hivedb-napi",
  "targets": [
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc"
  ]
}
```

### Scripts del paquete

| Script | Acción |
|---|---|
| `build:native` | `napi build --platform --release ...` + renombra `index.js` → `native.cjs` |
| `build` | `build:native` + `tsc` |
| `artifacts` | `napi artifacts` (coloca binarios en `npm/<triple>/`) |
| `create-npm-dirs` | `napi create-npm-dirs` (genera subpaquetes `npm/`) |
| `prepublishOnly` | `napi prepublish -t npm` + `tsc` |
| `preversion` | `napi build --platform` + `git add .` |
| `version` | `napi version` (sincroniza versión en subpaquetes) |

### CI (`.github/workflows/ci.yml`)

- **Job `lint-and-test`** (ubuntu): `cargo fmt`, `cargo clippy`, `cargo test`.
- **Job `build`** (matrix de 6 targets): compila cada binario con `napi build --platform --release --target <triple>`. Musl usa `-x` (cargo-zigbuild + Zig). El runner de macOS x64 se hace cross-compile desde `macos-latest` (ARM) — no se usa `macos-13` (Intel) porque GitHub lo está retirando.
- **Job `test-bun`** (ubuntu): descarga bindings linux-x64-gnu, compila TS, ejecuta `bun test`.
- **Job `publish`** (solo en tag `v*`): `create-npm-dirs` → `download-artifact` → `napi artifacts` → `npm publish` por subpaquete → `tsc` → `npm publish` principal.

### `.cargo/config.toml` (musl)

```toml
[target.x86_64-unknown-linux-musl]
rustflags = ["-C", "target-feature=-crt-static"]

[target.aarch64-unknown-linux-musl]
rustflags = ["-C", "target-feature=-crt-static"]
```

### Archivos generados (no commitear)

- `packages/hive-db/native.cjs` — loader multiplataforma generado por `napi build --platform`.
- `packages/hive-db/hivedb-napi.*.node` — binarios nativos por triple.
- `packages/hive-db/npm/` — subpaquetes por plataforma (regenerados por `napi create-npm-dirs`).
- `packages/hive-db/dist/` — salida de `tsc`.

Todos están en `.gitignore`.
