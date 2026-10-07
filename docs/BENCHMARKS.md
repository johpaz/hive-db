# Benchmarks

Medidas reproducibles de HiveDB frente a los motores embebidos con los que se compara
realmente en la capa vectorial: **sqlite-vec** y **LanceDB**. Las salidas crudas están en
[`docs/benchmarks/`](benchmarks/).

> **Léelos con cautela.** Los datos son **sintéticos** (vectores aleatorios agrupados en 50
> clusters), no embeddings reales, y se midió una sola máquina. Sirven para comparar
> motores entre sí con idénticos datos, no para predecir el recall que verás con tu corpus.
> Con embeddings reales el recall del ANN suele ser distinto (normalmente mejor).

## Entorno

| | |
|---|---|
| Fecha | 2026-10-07 |
| CPU / RAM | AMD Ryzen 9 6900HX, 16 hilos / 28 GB |
| SO | Linux 7.2 (Fedora) |
| Rust | 1.96.0, perfil `release` |
| HiveDB | commit `da81dd4` + corrección de los parámetros del HNSW (ver «Corrección del HNSW») |
| sqlite-vec | 0.1.9 (SQLite 3.51.2), búsqueda exacta (fuerza bruta) |
| LanceDB | 0.40.0; `flat` = exacto, `hnsw` = índice `IVF_HNSW_SQ`, métrica coseno |
| Datos | dim 384, k = 10, 1 000 consultas secuenciales, semilla fija |


## Resultados

### 100 000 documentos

| Motor | Ingesta (docs/s) | Vector p50 | Vector p99 | recall@10 | Arranque en frío | Disco | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| **HiveDB** (HNSW, M=24, ef=200) | 690 | 2,0 ms | 5,4 ms | 0,96 | 2 130 ms | 494,6 MiB | 942 MiB |
| sqlite-vec (exacto) | 109 436 | 65,5 ms | 67,6 ms | 1,00 | 67 ms | 149,4 MiB | 186 MiB |
| LanceDB flat (exacto) | 76 157 | 162,2 ms | 170,9 ms | 1,00 | 190 ms | 146,7 MiB | 1 512 MiB |
| LanceDB IVF_HNSW_SQ (defecto) | 11 995 | 1,8 ms | 2,4 ms | 0,51 | 79 ms | 203,2 MiB | 953 MiB |

HiveDB, además, en la misma base: texto (BM25) p50 0,87 ms; híbrido (BM25 + vector + RRF)
p50 3,0 ms / p99 6,7 ms; cierre limpio 2,2 s; recall idéntico (0,96) tras cerrar y reabrir.
Los parámetros por defecto de LanceDB dan poco recall; ajustado se compara abajo.

### 10 000 documentos

| Motor | Ingesta (docs/s) | Vector p50 | Vector p99 | recall@10 | Arranque en frío | Disco | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| **HiveDB** (HNSW, M=24, ef=200) | 769 | 2,3 ms | 3,1 ms | 0,99 | 180 ms | 56,0 MiB | 119 MiB |
| sqlite-vec (exacto) | 87 075 | 6,7 ms | 8,4 ms | 1,00 | 8 ms | 15,3 MiB | 53 MiB |
| LanceDB flat (exacto) | 9 885 | 9,5 ms | 15,4 ms | 1,00 | 25 ms | 14,7 MiB | 613 MiB |
| LanceDB IVF_HNSW_SQ (defecto) | 6 847 | 1,8 ms | 2,7 ms | 0,89 | 10 ms | 20,2 MiB | 359 MiB |

## Curva recall / latencia (misma base, 100 000 documentos)

### HiveDB (`efSearch`)

`ef` se fija por consulta (`efSearch` en TypeScript, `with_ef_search` en Rust; por defecto
200). Salida cruda en [`benchmarks/hivedb-100k.txt`](benchmarks/hivedb-100k.txt).

| `ef` | recall@10 | Vector p50 | Vector p99 |
|---:|---:|---:|---:|
| 50 | 0,73 | 1,1 ms | 2,8 ms |
| 100 | 0,89 | 1,5 ms | 4,1 ms |
| **200** (defecto) | 0,96 | 1,9 ms | 5,6 ms |
| 400 | 0,98 | 2,5 ms | 6,9 ms |
| 800 | 0,98 | 3,9 ms | 8,2 ms |

### LanceDB IVF_HNSW_SQ ajustado

Salida cruda en [`benchmarks/lancedb-hnsw-ajuste-100k.txt`](benchmarks/lancedb-hnsw-ajuste-100k.txt).

| `nprobes` | `ef` | `refine_factor` | recall@10 | Vector p50 |
|---:|---:|---:|---:|---:|
| 1 | defecto | — | 0,50 | 2,1 ms |
| 20 | 100 | — | 0,95 | 2,4 ms |
| 20 | 400 | — | 0,98 | 3,0 ms |
| 100 | 1 600 | — | 0,99 | 4,3 ms |
| 100 | 3 200 | 10 | 1,00 | 9,6 ms |

**A igual recall los dos están en el mismo orden de magnitud**: ~0,96–0,98 cuesta 1,9–2,5 ms
(p50) en HiveDB y 2,4–3,0 ms en LanceDB. La p99 de HiveDB es peor (5–7 ms frente a
~4 ms). Es una sola máquina y una sola ejecución: no hay base para declarar un ganador, sí
para decir que HiveDB ya no está en desventaja de búsqueda en estos datos.

## Corrección del HNSW

La primera ronda de benchmarks mostró a HiveDB con recall@10 de 0,43 a 100k y ~25 ms para
llegar a 0,89, un orden de magnitud peor que LanceDB ajustado. La causa era un **error del
propio motor**: `Hnsw::new(16, 100_000, 200, 16, …)` pasaba los argumentos en el orden
equivocado (`max_layer=200`, `ef_construction=16`), así que el índice se construía con
`ef_construction` = 16 en vez de 200. Se corrigió y se barrieron `M` y `ef_construction`
(100k, `ef`=200; salida en [`benchmarks/hivedb-ajuste-construccion-100k.txt`](benchmarks/hivedb-ajuste-construccion-100k.txt)):

| M | `ef_construction` | recall@10 (ef=200) | Ingesta (docs/s) |
|---:|---:|---:|---:|
| 16 | 16 (el error) | 0,47 | 3 281 |
| 16 | 64 | 0,75 | 1 164 |
| 16 | 200 | 0,90 | 520 |
| 24 | 64 | 0,93 | 961 |
| **24** | **100** (elegido) | 0,97 | 711 |
| 24 | 200 | 0,98 | 381 |
| 32 | 200 | 0,99 | 313 |

Se eligió M=24, `ef_construction`=100: casi todo el recall con el doble de ingesta que
`ef_construction`=200. El coste es la ingesta (de ~3,2k a ~0,7k docs/s, la inserción es
secuencial) y algo de disco. Las salidas anteriores a la corrección se conservan en
[`benchmarks/previo-fix-hnsw/`](benchmarks/previo-fix-hnsw/). Un grafo ya guardado en disco
sigue siendo válido: conserva los parámetros con que se construyó.

## Qué dicen (y qué no)

**A favor de HiveDB**
- Búsqueda vectorial aproximada de ~2 ms con recall 0,96 a 100k: ~33× más rápida que
  sqlite-vec exacto y ~80× que LanceDB exacto.
- A igual recall, latencia del mismo orden que LanceDB IVF_HNSW_SQ ajustado.
- Es el único de los comparados que da **búsqueda híbrida (BM25 + vector + RRF) en el mismo
  motor** (3,0 ms p50 a 100k), además del log de eventos, las proyecciones y el consent graph.

**En contra, sin maquillar**
- **Arranque en frío: 2,1 s a 100k** frente a 67–190 ms. Con el grafo HNSW persistido ya
  bajó de ~50 s a ~2 s; el resto es la carga de documentos al índice de texto.
- **Disco: 495 MiB** frente a 147–203 MiB. Duplica datos (log, documentos, índice de texto y
  grafo HNSW).
- **Ingesta: ~0,7k docs/s**, muy por debajo de sqlite-vec o LanceDB. No es una comparación
  limpia: HiveDB indexa texto (BM25), mantiene el log inmutable y construye HNSW al
  insertar (hoy de forma secuencial); los otros comparadores solo guardan el vector.
- **p99 vectorial** más alto que el de LanceDB ajustado.
- sqlite-vec y LanceDB flat son exactos (recall 1,0 por construcción): no son comparables
  en recall, solo en latencia.

## Competidores no medidos

Mem0, Zep/Graphiti y Letta son capas/servicios de memoria para agentes que dependen de un
LLM y de una base externa; no son comparables en latencia de búsqueda y **no se han
medido**. Comparación cualitativa según su documentación pública (no verificada en esta
máquina):

| | Enfoque | Dependencias típicas | Diferencia con HiveDB |
|---|---|---|---|
| **Mem0** | Capa de memoria que extrae «recuerdos» con un LLM | LLM + almacén vectorial externo | HiveDB es el almacén, no extrae: log inmutable, causalidad y consentimiento |
| **Zep / Graphiti** | Grafo de conocimiento temporal | Servicio + base de grafos + LLM | HiveDB es embebido, sin servidor; no construye grafo de entidades |
| **Letta** | Framework de agentes con memoria por bloques | Servidor + base de datos | HiveDB no es framework de agentes; es el motor que usarías debajo |
| **Turso (vectores)** | SQLite/libSQL con tipo vectorial | — | **Pendiente de medir** con su API embebida |

## Cómo reproducirlo

```bash
# HiveDB (genera y mide en proceso). Mismo corpus que los comparadores:
HIVE_BENCH_CLUSTERS=50 HIVE_BENCH_EXPORT=/ruta/corpus HIVE_BENCH_EF=50,200,800,3200 \
  cargo run --release -p hivedb-bench -- 100000 1000

# Comparadores (Python: numpy, psutil, sqlite-vec, lancedb). Un proceso por sistema:
python crates/hivedb-bench/comparadores/comparar.py /ruta/corpus sqlite-vec 1000
python crates/hivedb-bench/comparadores/comparar.py /ruta/corpus lancedb-flat 1000
python crates/hivedb-bench/comparadores/comparar.py /ruta/corpus lancedb-hnsw 1000
```

Para 10k, sustituye `100000` por `10000`. Los tiempos varían con la máquina; compara
siempre en la tuya.

## Embedder local (no vectorial)

Medido aparte, con `multilingual-e5-small` sobre `candle` en CPU: ~63 documentos/s al
indexar y ~54 ms por consulta de texto; el binario crece 5,9 MB (13,0 → 18,9 MB). Para
corpus grandes conviene aportar los vectores.

## Pendiente

- Repetir con embeddings reales de un corpus público es/en.
- Paralelizar la inserción por lotes en el HNSW (hoy secuencial) para recuperar ingesta.
- Medir Turso con vectores.
- Reducir arranque en frío y disco (ver «En contra»).
