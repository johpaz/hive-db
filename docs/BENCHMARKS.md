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
| HiveDB | commit `da81dd4` (+ cambios locales del embedder, que no afectan a estas rutas) |
| sqlite-vec | 0.1.9 (SQLite 3.51.2), búsqueda exacta (fuerza bruta) |
| LanceDB | 0.40.0; `flat` = exacto, `hnsw` = índice `IVF_HNSW_SQ`, métrica coseno |
| Datos | dim 384, k = 10, 1 000 consultas secuenciales, semilla fija |

## Resultados

### 100 000 documentos

| Motor | Ingesta (docs/s) | Vector p50 | Vector p99 | recall@10 | Arranque en frío | Disco | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| **HiveDB** (HNSW, ef=200) | 3 239 | 1,7 ms | 3,7 ms | 0,42 | 1 878 ms | 473,9 MiB | 876 MiB |
| sqlite-vec (exacto) | 109 436 | 65,5 ms | 67,6 ms | 1,00 | 67 ms | 149,4 MiB | 186 MiB |
| LanceDB flat (exacto) | 76 157 | 162,2 ms | 170,9 ms | 1,00 | 190 ms | 146,7 MiB | 1 512 MiB |
| LanceDB IVF_HNSW_SQ | 11 995 | 1,8 ms | 2,4 ms | 0,51 | 79 ms | 203,2 MiB | 953 MiB |

HiveDB, además, en la misma base: texto (BM25) p50 0,87 ms; híbrido (BM25 + vector + RRF)
p50 2,8 ms / p99 4,9 ms; cierre limpio 1,6 s; recall idéntico (0,42) tras cerrar y reabrir.

### 10 000 documentos

| Motor | Ingesta (docs/s) | Vector p50 | Vector p99 | recall@10 | Arranque en frío | Disco | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| **HiveDB** (HNSW, ef=200) | 6 752 | 0,9 ms | 1,7 ms | 0,87 | 187 ms | 54,2 MiB | 111 MiB |
| sqlite-vec (exacto) | 87 075 | 6,7 ms | 8,4 ms | 1,00 | 8 ms | 15,3 MiB | 53 MiB |
| LanceDB flat (exacto) | 9 885 | 9,5 ms | 15,4 ms | 1,00 | 25 ms | 14,7 MiB | 613 MiB |
| LanceDB IVF_HNSW_SQ | 6 847 | 1,8 ms | 2,7 ms | 0,89 | 10 ms | 20,2 MiB | 359 MiB |

## Curva recall / latencia de HiveDB (`efSearch`, 100 000 documentos)

`ef` se puede fijar por consulta (`efSearch` en TypeScript, `with_ef_search` en Rust; por
defecto 200). Misma base y mismos datos que arriba, salida cruda en
[`benchmarks/hivedb-ef-100k.txt`](benchmarks/hivedb-ef-100k.txt):

| `ef` | recall@10 | Vector p50 | Vector p99 |
|---:|---:|---:|---:|
| 50 | 0,27 | 0,7 ms | 1,4 ms |
| 100 | 0,35 | 1,1 ms | 2,4 ms |
| **200** (defecto) | 0,43 | 1,8 ms | 4,0 ms |
| 400 | 0,54 | 2,8 ms | 6,1 ms |
| 800 | 0,66 | 4,6 ms | 10,4 ms |
| 1 600 | 0,78 | 10,5 ms | 16,4 ms |
| 3 200 | 0,89 | 24,6 ms | 28,5 ms |

El recall sube de forma sostenida con `ef`, pero cada duplicación cuesta ~1,6–2,3× de
latencia. Con estos datos sintéticos (altísima dimensión intrínseca) llegar a ~0,9 exige
`ef` ≈ 3 200 (~25 ms), que sigue siendo ~3× más rápido que sqlite-vec exacto. El recall de
0,43 con `ef=200` difiere de 0,42 de la tabla principal porque el grafo se construye en
paralelo y no es determinista entre ejecuciones. LanceDB se midió con sus parámetros por
defecto, sin barrido equivalente de `nprobes`/`refine_factor`.

## Qué dicen (y qué no)

**A favor de HiveDB**
- La búsqueda vectorial aproximada es del orden de milisegundos y escala bien: a 100k es
  ~38× más rápida que sqlite-vec exacto y ~95× que LanceDB exacto.
- Con el mismo tipo de índice (HNSW), la latencia es equiparable a LanceDB (1,7 ms vs
  1,8 ms p50), y a 10k el recall es parecido (0,87 vs 0,89).
- Es el único de los tres que da **búsqueda híbrida (BM25 + vector + RRF) en el mismo
  motor** (2,8 ms p50 a 100k), además del log de eventos, las proyecciones y el consent graph.

**En contra, sin maquillar**
- **Recall a 100k: 0,42** frente a 0,51 de LanceDB HNSW. Ambos caen mucho en estos datos
  sintéticos; el de HiveDB es peor con los valores por defecto (ver la curva de `ef` abajo,
  y recuerda que LanceDB tampoco se ha ajustado).
- **Arranque en frío: 1,9 s a 100k** frente a 67–190 ms. Con el grafo HNSW persistido ya
  bajó de ~50 s a 1,9 s; el resto es la carga de documentos al índice de texto.
- **Disco: 474 MiB** frente a 147–203 MiB. Duplica datos (log, documentos, índice de texto y
  grafo HNSW).
- **Ingesta: 3,2k docs/s**, muy por debajo de sqlite-vec o LanceDB. No es una comparación
  limpia: HiveDB indexa texto (BM25), mantiene el log inmutable y construye HNSW al
  insertar; los otros comparadores solo guardan el vector.
- La **comparación de recall y arranque no es de manzanas con manzanas**: sqlite-vec y
  LanceDB flat son exactos (recall 1,0 por construcción).

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
- Barrido equivalente en LanceDB (`nprobes`, `refine_factor`) para una comparación a recall igual.
- Medir Turso con vectores.
- Reducir arranque en frío, disco e ingesta (ver «En contra»).
