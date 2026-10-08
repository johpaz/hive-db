# Cambios

Formato basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/). HiveDB está en 0.x:
la API puede cambiar entre versiones menores.

## Sin publicar

### Rendimiento (100.000 documentos de 384 dimensiones, k = 10)

| | Antes | Ahora |
|---|---:|---:|
| Apertura de una base poblada | ~50 s | **~40 ms** |
| Búsqueda vectorial p50 / p99 | 1,9 / 6,0 ms | **0,6–0,8 / 1,2–1,6 ms** |
| recall@10 (`efSearch` = 200) | 0,43 | **0,996** |
| Inserción por lotes | 3.200 docs/s | ~8.000 docs/s |
| Cierre limpio | 1,6 s | ~50 ms |
| Disco | 474 MiB | **204 MiB** |
| Binario nativo (linux-x64-gnu) | 13,0 MB | 10,0 MB |

Cifras con vectores sintéticos agrupados en una sola máquina; la metodología, las curvas y los
comparadores (sqlite-vec, LanceDB) están en [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

### Añadido
- **Embedder local opcional** (`hivedb-embed`): `embedder: "local"` genera los embeddings a partir del
  texto con `multilingual-e5-small` sobre `candle` (Rust puro, sin ONNX Runtime). Descarga verificada
  con SHA-256 a una revisión fija; `HIVEDB_OFFLINE=1` impide el acceso a la red. Feature de Cargo
  `embedder-local` en `hivedb-napi`, apagada por defecto.
- **`efSearch` por consulta** para ajustar precisión y velocidad del ANN (por defecto 200).
- **`hivedb-bench`**: benchmarks reproducibles y comparadores contra sqlite-vec y LanceDB.
- Licencia Apache-2.0 (`LICENSE`).
- `docs/AGENT_GUIDE.md`: guía de uso para runtimes de agentes (qué guardar dónde, memoria semántica, recetas, operación y plan de adopción por proyecto).
- Ejemplo `fetch_model` (`hivedb-embed`) para descargar y verificar el modelo de antemano, y procedimiento
  para instalaciones sin red en `docs/USER_GUIDE.md` §5.
- El benchmark mide la memoria anónima y la de ficheros mapeados por separado (`RssAnon` / `RssFile`).

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
- Las bases del formato anterior se **migran solas al abrir**, por tandas y de forma reanudable;
  `meta.json` no se modifica. Una base migrada **no puede abrirse con la versión anterior** (falla con
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
