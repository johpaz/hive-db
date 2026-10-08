# Benchmarks

Medidas reproducibles de HiveDB frente a los motores embebidos con los que se compara de verdad en
la capa vectorial: **sqlite-vec**, **LanceDB** y **libSQL** (el motor de Turso, en modo embebido).
Las salidas crudas están en [`docs/benchmarks/real/`](benchmarks/real/).

## Cómo se midió (léelo antes de las tablas)

- **Datos reales.** 100.000 frases de Wikipedia (WikiMatrix en-es: 50.000 en inglés y 50.000 en
  español, de 40 a 300 caracteres, sin traducciones directas entre sí) embebidas con
  `multilingual-e5-small` (384 dimensiones, normalizadas). 1.000 consultas: frases distintas de las
  del corpus (500 por idioma), embebidas como consulta. El ground truth de recall es la búsqueda
  exacta por fuerza bruta sobre esos mismos vectores. k = 10.
- **Disco real.** Las bases se escriben en un NVMe con btrfs, no en `/tmp`. (Las primeras mediciones de
  este proyecto escribían sin querer en `tmpfs`, es decir, en RAM, lo que falsea ingesta, cierre y
  arranque. Se descartaron: ver «Historial».)
- **Mismos vectores, mismo disco, consultas secuenciales,** un proceso por motor.
- **Memoria.** Se mide la memoria **anónima** del motor (montón) restando la del arnés de la prueba.
  Las páginas de ficheros mapeados (los vectores de HiveDB) se cuentan aparte porque el sistema las
  puede liberar. sqlite-vec y libSQL leen con `read()`, así que la caché de ficheros del sistema
  operativo no aparece en su cifra: la memoria anónima es la comparable entre motores.
- **Una sola máquina y una sola ejecución:** AMD Ryzen 9 6900HX (16 hilos), 28 GB, Linux 7.2.

| | |
|---|---|
| Fecha | 2026-10-07 |
| Rust | 1.96.0, perfil `release` |
| HiveDB | rama principal (HNSW propio, M=24, `ef_construction`=100) |
| sqlite-vec | 0.1.9 (SQLite 3.51.2), búsqueda exacta |
| LanceDB | 0.40.0; `flat` = exacto, `hnsw` = índice `IVF_HNSW_SQ`, coseno |
| libSQL | 0.1.x (Python), índice DiskANN `libsql_vector_idx`, coseno |

## Resultados con 100 000 documentos reales

| Motor | Ingesta (docs/s) | Vector p50 / p99 | recall@10 | Arranque en frío | Disco | Memoria anónima del motor |
|---|---:|---:|---:|---:|---:|---:|
| **HiveDB** (`ef`=200) | 5 220 | **1,4 / 1,8 ms** | 0,982 | **40 ms** | 241,2 MiB | **31 MiB** (+148 MiB mapeados) |
| **HiveDB** (`ef`=400) | 5 220 | 2,4 / 3,1 ms | **0,994** | 40 ms | 241,2 MiB | 31 MiB (+148 MiB mapeados) |
| sqlite-vec (exacto) | **88 142** | 71,1 / 73,9 ms | 0,9999 | 75 ms | **149,4 MiB** | ~1 MiB |
| LanceDB flat (exacto) | 65 488 | 162,6 / 177,7 ms | 0,9999 | 212 ms | **146,7 MiB** | 1 559 MiB |
| LanceDB IVF_HNSW_SQ (defecto) | 9 496 | 2,0 / 3,5 ms | 0,590 | 89 ms | 203,0 MiB | 668 MiB |
| libSQL DiskANN, `compress_neighbors=float8` | 79 | 10,3 / 23,9 ms | 0,975 | 27 ms | 2 758,5 MiB | 154 MiB |
| libSQL DiskANN, `compress_neighbors=float1bit` | 524 | 4,0 / 6,6 ms | 0,157 | 5 ms | 734,5 MiB | 153 MiB |

Además, en la misma base de 100k, HiveDB: búsqueda de texto (BM25) p50 0,8 ms / p99 2,0 ms —las
consultas son frases enteras—, búsqueda híbrida (texto + vector) p50 2,8 ms / p99 4,4 ms, cierre
limpio 44 ms, y mismo recall (0,982) tras cerrar y reabrir. Desglose del disco: vectores 146,5 MiB,
documentos (`semantic.redb`) 64,8 MiB, grafo 20,5 MiB, índice de texto 9,4 MiB.

**libSQL con los parámetros por defecto no se midió a 100k:** con 10 000 vectores ya ocupa 801 MiB
(~80 KB de índice por vector), así que 100k serían unos 8 GB y horas de construcción. Se midieron las
dos variantes comprimidas que ofrece. Con `float1bit` pierde casi toda la precisión (0,157); con
`float8` es utilizable (0,975) pero ocupa 2,7 GB y tarda ~21 minutos en indexar.

## Resultados con 10 000 documentos reales

Los 10 000 primeros documentos del mismo corpus.

| Motor | Ingesta (docs/s) | Vector p50 / p99 | recall@10 | Arranque en frío | Disco |
|---|---:|---:|---:|---:|---:|
| **HiveDB** (`ef`=200) | 5 992 | **0,47 / 0,85 ms** | 0,997 | 13 ms | 26,5 MiB |
| sqlite-vec (exacto) | 40 902 | 7,2 / 8,1 ms | 1,000 | 9 ms | 15,3 MiB |
| LanceDB flat (exacto) | 9 003 | 8,5 / 15,2 ms | 1,000 | 26 ms | 14,7 MiB |
| LanceDB IVF_HNSW_SQ (defecto) | 7 227 | 1,8 / 2,6 ms | 0,778 | 12 ms | 20,2 MiB |
| libSQL DiskANN (defecto) | 79 | 9,6 / 11,6 ms | 0,993 | 11 ms | 801,0 MiB |

## Precisión frente a velocidad

### HiveDB (`efSearch`, 100 000 documentos)

`ef` se fija por consulta (`efSearch` en TypeScript, `with_ef_search` en Rust; por defecto 200).
Salida cruda: [`benchmarks/real/hivedb-100k.txt`](benchmarks/real/hivedb-100k.txt).

| `ef` | recall@10 | Vector p50 | Vector p99 |
|---:|---:|---:|---:|
| 50 | 0,858 | 0,4 ms | 0,7 ms |
| 100 | 0,943 | 0,8 ms | 1,2 ms |
| **200** (defecto) | 0,982 | 1,4 ms | 1,9 ms |
| 400 | 0,994 | 2,4 ms | 3,1 ms |
| 800 | 0,998 | 3,9 ms | 5,1 ms |

Con 10 000 documentos el mismo barrido da 0,966 → 0,9999 entre `ef`=50 y 800, con p50 de 0,12 a 1,0 ms.

### LanceDB IVF_HNSW_SQ ajustado (100 000 documentos)

Salida cruda: [`benchmarks/real/lancedb-hnsw-ajuste-100k.txt`](benchmarks/real/lancedb-hnsw-ajuste-100k.txt).

| `nprobes` | `ef` | `refine_factor` | recall@10 | Vector p50 / p99 |
|---:|---:|---:|---:|---:|
| 1 | defecto | — | 0,590 | 1,8 / 2,6 ms |
| 20 | 100 | — | 0,919 | 2,3 / 3,3 ms |
| 20 | 400 | — | 0,965 | 3,1 / 4,0 ms |
| 50 | 800 | — | 0,970 | 4,7 / 8,3 ms |
| 100 | 1 600 | — | 0,972 | 6,3 / 8,6 ms |
| 100 | 3 200 | 10 | 0,9998 | 9,9 / 13,1 ms |

**A igual recall, HiveDB es más rápido** en estos datos: ~0,98 en 1,4 ms frente a ~0,965 en 3,1 ms de
LanceDB; ~0,998 en 4,0 ms frente a 0,9998 en 9,9 ms. Con los parámetros por defecto, LanceDB tiene una
latencia parecida pero un recall de 0,59. Es una máquina, una ejecución y 100k documentos: léelo como
«sin desventaja de búsqueda», no como una garantía general.

## Qué dicen (y qué no)

**A favor de HiveDB**
- La búsqueda vectorial aproximada con recall ~0,98 tarda ~1,4 ms con 100.000 documentos reales: unas
  50 veces menos que sqlite-vec exacto y más de 100 veces menos que LanceDB exacto.
- A igual recall, más rápida que LanceDB IVF_HNSW_SQ ajustado, y mucho más precisa que sus parámetros
  por defecto.
- Arranque en frío de 40 ms, del orden del de sqlite-vec (75 ms) y LanceDB (89–212 ms).
- Memoria anónima del motor de ~31 MiB: los 146 MiB de vectores van mapeados desde disco y el sistema
  los puede liberar.
- Frente a libSQL (Turso embebido): precisión similar a su mejor variante con 11 veces menos disco y
  ~60 veces más ingesta, y una latencia ~7 veces menor.
- Es el único de los comparados que da **búsqueda híbrida (BM25 + vector + RRF) en el mismo motor**,
  además del log de eventos, las proyecciones y el consentimiento.

**En contra, sin maquillar**
- **Ingesta: ~5,2k docs/s**, frente a 88k de sqlite-vec y 9,5k–65k de LanceDB. HiveDB indexa además el
  texto, mantiene el log y construye el grafo al insertar; los comparadores solo guardan el vector.
- **Disco: 241,2 MiB** frente a 147–149 MiB de sqlite-vec y LanceDB exacto (~1,6×). Los documentos de
  texto real y el índice BM25 pesan 74 MiB.
- **recall@10 de 0,982 con el `ef` por defecto,** no 1,0: para ≥ 0,99 hay que subir `efSearch` a 400
  (2,4 ms).
- sqlite-vec y LanceDB flat son exactos por construcción (recall 1,0): no son comparables en recall,
  solo en latencia.
- **Alcance:** 100.000 documentos, una máquina. No se ha probado con millones, en otros sistemas
  operativos ni con muchos clientes concurrentes.

## Historial: cómo se llegó aquí

Esta sección conserva los cambios y sus efectos. **Las cifras de aquí son sintéticas** (vectores
aleatorios agrupados en 50 clusters) y se midieron cuando el directorio de trabajo estaba en `tmpfs`
(RAM): sirven para comparar «antes y después» entre sí, no como cifras absolutas (en particular, la
ingesta y el cierre estaban inflados por no haber `fsync` real). Las tablas de arriba son las vigentes.

### Un error en los parámetros del HNSW

La primera ronda dejó a HiveDB con recall@10 de 0,43 a 100k. La causa era un **error del motor**:
`Hnsw::new(16, 100_000, 200, 16, …)` de la crate `hnsw_rs` recibe `(M, max_elements, max_layer,
ef_construction)`; los argumentos iban en otro orden y el índice se construía con `ef_construction` =
16 en vez de 200. Se corrigió y se barrieron `M` y `ef_construction` (100k sintéticos, `ef`=200;
[salida](benchmarks/hivedb-ajuste-construccion-100k.txt)):

| M | `ef_construction` | recall@10 | Ingesta secuencial (docs/s) |
|---:|---:|---:|---:|
| 16 | 16 (el error) | 0,47 | 3 281 |
| 16 | 64 | 0,75 | 1 164 |
| 16 | 200 | 0,90 | 520 |
| 24 | 64 | 0,93 | 961 |
| **24** | **100** (elegido) | 0,97 | 711 |
| 24 | 200 | 0,98 | 381 |
| 32 | 200 | 0,99 | 313 |

### Optimizaciones sucesivas (100k sintéticos)

| Cambio | Efecto medido |
|---|---|
| Inserción por lotes en paralelo | Ingesta 0,7k → 5,1k docs/s, mismo recall |
| Distancia coseno propia: producto punto en `f32` con 16 acumuladores y vectores normalizados, en vez de `DistCosine` de `anndists` (tres acumuladores `f64` por par, sin vectorizar) | Vector p50 1,95 → 1,3 ms; p99 6,0 → ~4 ms |
| Índice de texto persistido con marcador de generación y sin releer los documentos | Arranque 2,2 s → 0,9–1,3 s |
| **HNSW propio de almacenamiento plano** (`flat_hnsw`), que sustituye a `hnsw_rs`: arrays `u32` para el grafo, vectores contiguos mapeados con `mmap`, construcción por tandas paralela y determinista | Arranque ~1 s → ~40 ms; vector p50 1,3 → 0,6 ms; p99 ~4 → 1,2 ms; recall@10 0,96 → 0,996; disco 495 → 429 MiB |
| **Una sola copia de los vectores**: fichero plano autoritativo (normalizado); `redb` solo guarda la ranura de cada documento; el grafo lee del mismo fichero mapeado | Disco 429 → 204 MiB |

`hnsw_rs` reconstruía un objeto por vecino al cargar (~0,9 s con 100k vectores, incluso con `mmap`) y
guardaba cada lista de vecinos tras un `Arc<RwLock<…>>`. El grafo plano se lee en bloque y se recorre sin
punteros. Además desaparecen el `Box::leak` y el `catch_unwind` que `hnsw_rs` obligaba a usar (el motor
propio valida los ficheros al cargar y no entra en pánico). Detalle en
[`IMPLEMENTATION.md`](IMPLEMENTATION.md) §7. Con datos reales el recall del mismo motor es algo menor
(0,982 frente a 0,996 sintético a `ef`=200): los embeddings reales son más difíciles de indexar.

Las salidas de esa época están en [`benchmarks/`](benchmarks/) (sintéticas, `tmpfs`) y en
[`benchmarks/previo-fix-hnsw/`](benchmarks/previo-fix-hnsw/) (antes de corregir el error de los parámetros).

### Búsqueda de texto e híbrida (cambios de 0.6.1)

La búsqueda de texto con frases enteras como consulta era la más lenta de HiveDB (p50 2,5 ms, p99 7,0 ms)
y la híbrida llegaba a p99 9,5 ms. Dos cambios en el análisis y la fusión (ver
[`IMPLEMENTATION.md`](IMPLEMENTATION.md) §7) la dejaron en texto p50 **0,8 ms** / p99 2,0 ms e híbrida p50
**2,8 ms** / p99 4,4 ms con el mismo corpus real de 100k:

- **Palabras vacías** (español e inglés) fuera del índice y de las consultas: «de», «la», «the»… generaban
  miles de coincidencias por consulta, y además en la fusión RRF una coincidencia de «de» valía como un
  primer puesto. El índice de texto también pesa menos (10,8 → 9,4 MiB).
- **Fusión con más candidatos que `k`** y **desempate determinista**: antes, con `k` pequeño, dos documentos
  con la misma puntuación RRF salían en orden aleatorio.

## Competidores no medidos

Mem0, Zep/Graphiti y Letta son capas o servicios de memoria para agentes que dependen de un LLM y de
una base externa; no son comparables en latencia de búsqueda y **no se han medido**. Comparación
cualitativa según su documentación pública (no verificada en esta máquina):

| | Enfoque | Dependencias típicas | Diferencia con HiveDB |
|---|---|---|---|
| **Mem0** | Capa de memoria que extrae «recuerdos» con un LLM | LLM + almacén vectorial externo | HiveDB es el almacén, no extrae: log inmutable, causalidad y consentimiento |
| **Zep / Graphiti** | Grafo de conocimiento temporal | Servicio + base de grafos + LLM | HiveDB es embebido, sin servidor; no construye grafo de entidades |
| **Letta** | Framework de agentes con memoria por bloques | Servidor + base de datos | HiveDB no es framework de agentes; es el motor que usarías debajo |

Turso se midió en su modo embebido (libSQL sobre un fichero local), no Turso Cloud: una consulta por red
sumaría la latencia de la conexión y no sería comparable con un motor que corre en el mismo proceso.

## Cómo reproducirlo

```bash
# 1. Corpus real: textos (uno por línea) → vectores f32 con el embedder local.
cargo run --release -p hivedb-embed --example embed_corpus -- textos.txt corpus/vectors.f32 doc
cargo run --release -p hivedb-embed --example embed_corpus -- consultas.txt corpus/queries.f32 query
cp textos.txt corpus/texts.txt; cp consultas.txt corpus/queries.txt

# 2. HiveDB sobre ese corpus (¡un directorio en disco real, no en tmpfs!).
TMPDIR=/ruta/en/disco HIVE_BENCH_CORPUS=corpus HIVE_BENCH_EF=50,100,200,400,800 \
  cargo run --release -p hivedb-bench -- 100000 1000

# 3. Comparadores (Python: numpy, sqlite-vec, lancedb, libsql). Un proceso por sistema:
TMPDIR=/ruta/en/disco python crates/hivedb-bench/comparadores/comparar.py corpus sqlite-vec 1000
python crates/hivedb-bench/comparadores/comparar.py corpus lancedb-flat 1000
python crates/hivedb-bench/comparadores/comparar.py corpus lancedb-hnsw 1000   # LANCE_NPROBES / LANCE_EF / LANCE_REFINE ajustan la búsqueda
LIBSQL_COMPRESS=float8 python crates/hivedb-bench/comparadores/comparar.py corpus libsql 1000
```

Sin `HIVE_BENCH_CORPUS` el benchmark genera vectores sintéticos (`HIVE_BENCH_CLUSTERS=50` los agrupa).
Para 10k, usa los 10 000 primeros vectores del corpus. Los tiempos varían con la máquina: compara siempre
en la tuya, y comprueba con `df -T` que `TMPDIR` no es `tmpfs`.

## Embedder local (no vectorial)

Medido aparte, con `multilingual-e5-small` sobre `candle` en CPU (16 hilos): ~47 documentos/s al indexar
frases de Wikipedia (~130 caracteres) y ~50 ms por consulta de texto. Los textos de cada lote se procesan
por longitud para no rellenar de más (de 28 a 47 textos/s con frases de longitud mezclada). Para corpus
grandes conviene aportar los vectores.

## Pendiente

- Repetir en otras máquinas y sistemas operativos (Windows y macOS sin ejercitar).
- Más de 100.000 documentos y clientes concurrentes.
- Medir Mem0, Zep y Letta con una tarea comparable de extremo a extremo (no se miden por latencia).
