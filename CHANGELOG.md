# Cambios

Formato basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/). HiveDB está en 0.x:
la API puede cambiar entre versiones menores.

## Sin publicar

## 0.7.0 — 2026-10-08

### Añadido
- **Binding Python: paquete `johpaz-hive-db` en PyPI** (el equivalente al scope `@johpaz/` de npm) (`import hivedb`). Mismo motor y mismas garantías que el
  paquete npm: registro de eventos, colecciones con versión optimista, búsqueda híbrida, suscripciones y
  embedder local (descarga del modelo al activarlo; un modelo por proceso). API síncrona que suelta el GIL
  —varios hilos consultan la misma base a la vez— más `AsyncHiveDB` con `asyncio`; resultados como
  `dataclasses`, stubs `.pyi` y errores `HiveDBError` con atributo `code`. Wheels `abi3` (Python ≥ 3.9) para
  Linux glibc/musl x64 y arm64, macOS x64/arm64 y Windows x64. Medido: 16 consultas en 16 hilos 146 ms (751 ms
  en serie), 815 MiB con 4 bases abiertas.
- **`johpaz-langchain-hivedb`** (paquete aparte, Python puro): `HiveDBVectorStore` (pasa la suite de contrato de
  `langchain-tests`), `HiveDBChatMessageHistory` y `HiveDBStore`, el `BaseStore` de LangGraph para memoria a
  largo plazo con búsqueda por significado (`graph.compile(store=HiveDBStore(db))`). Un agente de LangGraph
  recuerda tras reabrir la base. Todavía no incluye un checkpointer de LangGraph.
- **CI y publicación:** wheels de las seis plataformas, `sdist`, `pytest` en Python 3.9 y 3.13, pruebas con el
  modelo real en cinco plataformas (musl en Alpine), pruebas de los adaptadores y `publish-pypi` idempotente
  (Trusted Publishing); `publish` de npm espera a todo ello. `scripts/release.sh` publica npm y PyPI con la
  misma versión (la toma de `Cargo.toml`).

### Cambiado
- **Crate común `hivedb-binding-core`:** validación de eventos, patrones y consultas, DTOs, apertura, el candado
  de lectura/escritura y los errores con código salen de `hivedb-napi` a un crate sin dependencias del lenguaje
  anfitrión, compartido por el binding Node y el de Python. Sin cambios de comportamiento en Node (los tests de
  Bun, incluido el E2E del embedder, pasan igual); lo único visible es que los JSON de `causalThread`,
  `buildAgentContext` y `evaluateHarness` pueden salir con las claves en otro orden.

### Corregido
- **Preguntas en español:** las interrogativas con acento («cómo», «cuál», «dónde», «cuándo», «quién»…) contaban
  como coincidencia de texto porque la lista oficial de palabras vacías solo trae las formas sin acento. Una
  pregunta como «¿cómo cocinar arroz?» casaba con cualquier documento que dijera «Cómo configurar…» y, en la fusión
  RRF, ese primer puesto de texto podía desplazar a la coincidencia semántica correcta. Ahora se ignoran.

### Formato en disco y migración
- El análisis de texto cambia otra vez (versión 3), así que el índice de texto de una base existente se
  **reconstruye una vez al abrir**; los datos no se tocan.

## 0.6.1 — 2026-10-08

### Rendimiento
- **Memoria:** con el embedder local, 4 bases abiertas pasan de 2.939 MiB a **813 MiB** (el modelo se carga una vez;
  ~12 MiB por base adicional).
- **Concurrencia:** 16 consultas simultáneas con embedding sobre una base pasan de 742 ms a **119 ms** (≈130 consultas/s).
- **Búsqueda de texto y híbrida** (100k frases reales): texto p50 2,5 → **0,8 ms**, híbrida p50 4,3 → **2,8 ms**
  (p99 9,5 → 4,4 ms); índice de texto de 10,8 a 9,4 MiB; inserción por lotes ~5.200 docs/s.

### Añadido
- **`HiveDB.prepareEmbedder({ onProgress })`**: descarga y verifica el modelo del embedder local sin abrir
  ninguna base, con progreso (`{ file, fileIndex, fileCount, downloaded, total }`), y devuelve
  `{ dir, spaceId, cached }`. Evita la espera silenciosa de ~470 MB en la primera apertura.
- **Descarga del modelo robusta:** tiempos máximos, hasta 4 reintentos con espera creciente, **reanudación** con
  `Range` desde donde se cortó, bloqueo entre procesos y verificación de SHA-256. `HIVEDB_MODEL_BASE_URL`
  permite usar un espejo. Tests con un servidor HTTP local y contra Hugging Face real.
- **CI:** el test de extremo a extremo del embedder con el modelo real corre ahora en linux x64 glibc, linux
  arm64, macOS arm64, Windows y linux x64 musl (Alpine), y `publish` depende de toda la matriz.
- Guía de uso multiusuario (`docs/AGENT_GUIDE.md` §5.7): un modelo por aplicación, y cuándo usar una base compartida
  con filtro de inquilino o una base por inquilino.

### Corregido
- **Un modelo por aplicación, no por base:** cada `open({ embedder: "local" })` cargaba su propia copia del
  modelo (~735 MiB), así que 4 bases ocupaban 2,9 GB. Ahora todas las bases del proceso comparten una sola
  instancia (`LocalEmbedder::shared`): 813 MiB con 4 bases (~12 MiB por base adicional).
- **Las operaciones sobre una misma base se serializaban:** el binding mantenía un `Mutex` durante toda la
  operación, de modo que 16 consultas concurrentes tardaban lo mismo que 16 en fila (742 ms). Ahora el candado
  es de lectura/escritura y se reparten por los núcleos: 119 ms (~130 consultas/s con embedding).
- **Búsqueda híbrida no determinista:** con la misma puntuación RRF (muy frecuente, p. ej. con `k = 1`), el orden
  dependía del orden de un `HashMap` y cambiaba entre ejecuciones. Ahora el desempate es total y reproducible
  (puntuación, mejor puesto en alguna lista, id). Era la causa del fallo intermitente del test E2E del embedder.
- **La fusión solo miraba `k` candidatos por fuente,** de modo que un documento 2.º en una lista y 1.º en la otra
  empataba con los que solo eran 1.º en una. Ahora cada fuente aporta `max(5·k, 50)` candidatos antes de fusionar.
- **Palabras vacías:** «de», «la», «que», «the», «of»… contaban como coincidencias de texto (y en RRF como un
  primer puesto), desplazando la coincidencia semántica. Ahora se ignoran en español e inglés.
- Lints de `clippy` 1.99 (`as_chunks` en lugar de `chunks_exact` con tamaño constante).

### Formato en disco y migración
- **Desde 0.6.0:** el análisis de texto cambió (palabras vacías), así que el índice de texto de una base existente
  se **reconstruye una vez al abrir**; los datos no se tocan. (Lo gobierna `FTS_ANALYSIS_VERSION`, guardado en
  `fts.generation`.)

## 0.6.0 — 2026-10-07

### Rendimiento

**Cifras actuales,** con 100.000 frases reales de Wikipedia (español e inglés, `multilingual-e5-small`,
384 dimensiones) en disco NVMe y una sola máquina:

| | Valor |
|---|---:|
| Búsqueda vectorial p50 / p99 (`efSearch` = 200) | 1,4 / 1,8 ms |
| recall@10 (`efSearch` = 200 / 400) | 0,982 / 0,994 |
| Búsqueda de texto / híbrida p50 | 2,5 ms / 4,3 ms |
| Apertura de una base poblada | 46 ms |
| Cierre limpio | 42 ms |
| Inserción por lotes | ~4.900 docs/s |
| Disco | 242,5 MiB |
| Memoria anónima del motor | ~31 MiB (+148 MiB de vectores mapeados) |
| Binario nativo (linux-x64-gnu, sin el embedder) | 13,0 → 10,0 MB |

**Mejora respecto a la versión anterior,** medida con los mismos datos sintéticos (100k vectores
agrupados): la apertura de una base poblada pasó de ~50 s a decenas de milisegundos, el p99 de la
búsqueda vectorial de ~6 ms a ~1,2 ms, el recall@10 de 0,43 a 0,996 (por un error de parámetros, ver
«Corregido») y el disco de 474 MiB a 204 MiB. Esas cifras sintéticas se midieron sin querer sobre
`tmpfs`; la ingesta y el cierre de esa época no son comparables, por eso arriba solo van las reales.

Comparación con sqlite-vec, LanceDB y libSQL (Turso embebido) y metodología en
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

### Añadido
- **Embedder local opcional, incluido en los paquetes publicados** (`hivedb-embed`; binarios de 13,7–16,8 MB según la plataforma; los pesos del modelo no viajan en el paquete) : `embedder: "local"` genera los embeddings a partir del
  texto con `multilingual-e5-small` sobre `candle` (Rust puro, sin ONNX Runtime). Descarga verificada
  con SHA-256 a una revisión fija; `HIVEDB_OFFLINE=1` impide el acceso a la red. Feature de Cargo
  `embedder-local` en `hivedb-napi`, apagada por defecto.
- **`efSearch` por consulta** para ajustar precisión y velocidad del ANN (por defecto 200).
- **`hivedb-bench`**: benchmarks reproducibles y comparadores contra sqlite-vec, LanceDB y libSQL.
- Licencia Apache-2.0 (`LICENSE`).
- `docs/AGENT_GUIDE.md`: guía de uso para runtimes de agentes (qué guardar dónde, memoria semántica, recetas, operación y plan de adopción por proyecto).
- Ejemplo `fetch_model` (`hivedb-embed`) para descargar y verificar el modelo de antemano, y procedimiento
  para instalaciones sin red en `docs/USER_GUIDE.md` §5.
- El benchmark mide la memoria anónima y la de ficheros mapeados por separado (`RssAnon` / `RssFile`), lee un corpus real ya embebido (`HIVE_BENCH_CORPUS`) y los comparadores incluyen libSQL (Turso embebido).

### Cambiado
- **HNSW propio de almacenamiento plano** en lugar de `hnsw_rs`: arranque en milisegundos, construcción
  en paralelo y determinista, sin `panic!` ante ficheros corruptos. Ver `docs/IMPLEMENTATION.md` §7.
- **Una sola copia de los vectores.** Viven en un fichero plano (`vectors.N.dat`), normalizados, y
  `semantic.redb` solo guarda la ranura de cada documento; el grafo lee del mismo fichero mapeado.
- **Índice de texto y grafo persistidos** con marcadores de generación: si el cierre fue limpio, abrir
  no lee ningún documento.
- **Distancia coseno vectorizable** (producto punto en `f32` con acumuladores independientes).
- El embedder local ordena los textos por longitud dentro de cada lote (~1,7× más rápido con frases
  de longitud mezclada).
- README reescrito como presentación del proyecto; el historial de gates pasa a `docs/GATES.md`.
- Nombres unificados: producto `HiveDB`, paquete `@johpaz/hive-db`, crates `hivedb-*`, repositorio `hive-db`.

### Formato en disco y migración
- Las bases del formato anterior se **migran solas al abrir**: se escribe un fichero `redb` nuevo y solo
  se sustituye al terminar, así que una interrupción deja la base original intacta y se repite;
  `meta.json` no se modifica. Probado con copias de bases reales de Hive (los resultados de búsqueda
  coinciden, salvo el orden entre documentos con la misma puntuación por la actualización de tantivy). Una base migrada **no puede abrirse con la versión anterior** (falla con
  un error de tipo en lugar de ver un índice vacío): haz una copia antes de actualizar si necesitas
  poder volver atrás. Detalle en `docs/IMPLEMENTATION.md` §7 y `docs/USER_GUIDE.md` §5.
- Los vectores ya no conservan su magnitud original (la métrica es coseno).
- Los objetivos big-endian no compilan (`compile_error!`): los vectores se mapean sin convertir y se
  corromperían en silencio.

### Corregido
- **Recall del ANN:** el índice se construía con `ef_construction = 16` en lugar de 200 por pasar los
  argumentos de `Hnsw::new` en otro orden. El recall a 100k pasó de 0,43 a 0,96 solo con corregirlo.
- `HiveDB::read(seq)` devolvía `NotFound` tras reabrir una base cerrada limpiamente.
- Un fallo a mitad de una tanda de vectores ya no deja vectores huérfanos: al abrir se recortan a lo
  confirmado en `redb`.
