# HiveDB

**La base de datos de memoria para agentes de IA, embebida en tu proceso.**

HiveDB guarda lo que un agente hace, sabe y tiene permitido hacer —eventos, hechos, documentos,
permisos— y se lo devuelve cuando lo necesita: por orden, por palabras, por significado o por
causa. Corre dentro de tu aplicación (Bun/Node o Rust), en un único directorio, sin servidor y
sin depender de ningún servicio externo.

```ts
import { HiveDB } from "@johpaz/hive-db";

const db = await HiveDB.open("./data/mi-agente", { embedder: "local" });

// Lo que el agente hace: un log inmutable. El motor asigna el orden.
await db.append({
  agentId: "asistente",
  streamId: "viaje-a-paris",
  kind: "Fact",
  payload: JSON.stringify({ presupuesto: 1200 }),
});

// Lo que el agente sabe: búsqueda por palabras y por significado a la vez.
await db.upsertDoc({ id: "politica", body: "Cómo solicitar la devolución de dinero" });
const hits = await db.queryHybrid({ text: "reembolso", k: 3 }); // encuentra "politica"

// Lo que el agente puede hacer: permisos con auditoría.
const decision = await db.can("asistente", "read", "viajes/paris");
```

## Por qué HiveDB

Un agente que dura más que una conversación necesita memoria de verdad, no un `JSON` en disco ni
una base vectorial pegada a otra base de datos. HiveDB junta en un solo motor lo que ese agente
suele acabar montando con tres o cuatro piezas:

| Necesidad del agente | En HiveDB |
|---|---|
| **Recordar qué pasó y en qué orden** | Event-log append-only e inmutable. El estado actual es una *proyección* determinista: se puede reconstruir reproduciendo el log. |
| **Recuperar lo relevante** | Búsqueda híbrida: BM25 (con stemming en español) + vectorial (HNSW) + fusión por rango (RRF), en una sola consulta. |
| **Poder dar explicaciones** | Cada evento puede apuntar a su causa (`causation`). El *hilo causal* reconstruye por qué el agente decidió algo y detecta bucles de error. |
| **Limitar lo que hace** | Grafo de consentimiento: delegaciones con alcance y expiración, y un evento de auditoría por cada consulta de permiso. |
| **Reaccionar** | Suscripciones push, no polling. |
| **Guardar datos mutables** | Colecciones de documentos con versionado optimista, índices secundarios y lotes atómicos. |
| **Contexto para el LLM** | `buildAgentContext` arma ventanas de contexto que nunca exceden el límite de tokens, comprimiendo las fases terminadas. |
| **Memoria de trabajo** | Clave-valor en RAM con TTL. |

Los embeddings, a tu elección: con `embedder: "local"` (**recomendado**) el motor los genera a partir
del texto con `multilingual-e5-small` (español e inglés, en CPU, sin enviar nada fuera); o los
calculas tú con el modelo o la API que prefieras (OpenAI, Cohere, Voyage, un servidor propio…) y se
los pasas en `vector`. Con una API, el texto sale hacia ese proveedor: por eso recomendamos el local.

## Rendimiento

Medido con 100.000 documentos de 384 dimensiones, k = 10, una sola máquina (Ryzen 9 6900HX).
Los vectores son sintéticos y agrupados; el detalle, las curvas y los comandos para reproducirlo
están en [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

| | Vector p50 | recall@10 | Arranque en frío | Disco | Búsqueda híbrida |
|---|---:|---:|---:|---:|:---:|
| **HiveDB** (HNSW) | 0,6–0,8 ms | 0,996 | 39 ms | 204 MiB | Sí (1,6–1,9 ms p50) |
| sqlite-vec (exacto) | 65,5 ms | 1,00 | 67 ms | 149 MiB | No |
| LanceDB (exacto) | 162,2 ms | 1,00 | 190 ms | 147 MiB | No |
| LanceDB (IVF_HNSW_SQ, ajustado) | 2,4 ms | 0,95 | 79 ms | 203 MiB | No |

A igual recall la búsqueda vectorial de HiveDB es más rápida que la de LanceDB ajustado.
Honestidad por delante: HiveDB **ocupa ~1,4× el disco de sqlite-vec e ingiere más despacio
(~8k docs/s frente a 12k–109k)**, porque además indexa texto, mantiene el log y construye el grafo
al insertar. Son datos sintéticos de una máquina: con embeddings reales el recall puede diferir.

## Cómo se compara

| | Enfoque | Dependencias típicas |
|---|---|---|
| **HiveDB** | Motor embebido: log causal, estado, consentimiento, búsqueda híbrida | Ninguna (un proceso); embedder local opcional |
| **Mem0** | Capa que extrae recuerdos con un LLM | LLM + almacén vectorial externo |
| **Zep / Graphiti** | Grafo de conocimiento temporal | Servicio + base de grafos + LLM |
| **Letta** | Framework de agentes con memoria por bloques | Servidor + base de datos |
| **LanceDB / sqlite-vec** | Almacén vectorial embebido | Ninguna; sin log causal ni consentimiento |
| **Turso (vectores)** | SQLite/libSQL con tipo vectorial | Pendiente de medir |

La comparación cualitativa sale de la documentación pública de cada proyecto, no de mediciones
propias. HiveDB no extrae recuerdos con un LLM ni es un framework de agentes: es el motor de
memoria que se usaría debajo de uno.

## Instalación

```bash
bun add @johpaz/hive-db        # o: npm install @johpaz/hive-db / pnpm add @johpaz/hive-db
```

Trae binarios precompilados para Linux x64 (glibc y musl), Linux arm64, macOS x64/arm64 y
Windows x64: no necesitas Rust instalado. La guía de uso, con ejemplos de cada API, está en
[`docs/USER_GUIDE.md`](docs/USER_GUIDE.md).

> **Embedder local.** `embedder: "local"` necesita un binario compilado con la feature
> `embedder-local` (+6 MB). El modelo (~470 MB) no viaja con el paquete: se descarga la primera vez
> que lo activas, la única conexión de red del motor; con `HIVEDB_OFFLINE=1` nunca la hace y se puede preparar de antemano para máquinas
> sin red. Los paquetes publicados todavía no incluyen la feature; mientras tanto puedes aportar
> tus propios vectores, de un modelo local o de una API (`vector: { dimension, spaceId }`), o compilar
> el binding con `--features embedder-local`. Ver la sección 5 de la guía.

## Arquitectura

```
┌──────────────────────────────────────────────────────────┐
│  @johpaz/hive-db (TypeScript)   open · append · query …  │
└────────────────────────────┬─────────────────────────────┘
                             │ napi-rs
┌────────────────────────────┴─────────────────────────────┐
│  Núcleo Rust                                             │
│                                                          │
│  Event log ──▶ Proyecciones ──▶ Motor reactivo           │
│  (un shard      (hechos, tareas,   (suscripciones push)  │
│   redb por       herramientas,                           │
│   agente)        consentimiento)                         │
│                                                          │
│  Colecciones     Memoria de trabajo      Harness causal  │
│  (redb)          (RAM + TTL)             (hilo, contexto)│
│                                                          │
│  Memoria semántica híbrida                               │
│    texto: tantivy (BM25)                                 │
│    vector: HNSW propio sobre un fichero plano mapeado    │
│    fusión: RRF        embedder opcional: candle          │
└──────────────────────────────────────────────────────────┘
```

Todo vive en un directorio: `redb` para el log, las colecciones y los documentos; un fichero
plano con los vectores; y los índices derivados (texto y grafo), que se reconstruyen solos si
faltan. El log es la fuente de verdad. Los detalles del formato, la recuperación tras un fallo y
la migración están en [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md).

## Principios de diseño

1. **Soberanía digital:** cero dependencia de servicios externos en funcionamiento. Única excepción, opcional y explícita: el embedder local descarga el modelo (~470 MB) una vez, a una revisión fija verificada con SHA-256; después no necesita red, y la descarga se puede hacer de antemano para instalaciones aisladas (ver la guía, §5).
2. **El log es la fuente de verdad:** todo estado es una proyección derivada y reproducible.
3. **Embebido:** corre dentro del proceso consumidor, sin daemon.
4. **Agent-native:** consentimiento, memoria y reactividad viven en el motor, no se simulan por encima.
5. **Determinismo:** el mismo log produce el mismo estado.
6. **`unsafe` mínimo:** solo en las fronteras de mmap y FFI.

## Estado del proyecto

Versión **0.5.x**, antes de la 1.0: la API puede cambiar entre versiones menores, y el formato en
disco se migra solo al abrir (una base ya migrada no se puede abrir con una versión anterior; ver
la guía). El motor tiene tests de propiedades, de concurrencia con `loom` y de recuperación tras
fallos a medias. El historial de hitos está en [`docs/GATES.md`](docs/GATES.md).

Pendiente antes de la 1.0: benchmarks con embeddings reales, incluir el embedder local en los
paquetes publicados y estabilizar el formato de eventos en disco.

## Documentación

| | |
|---|---|
| [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md) | Guía de uso desde Bun/TypeScript, con ejemplos de cada API. |
| [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md) | Manual de implementación: formato en disco, índices, recuperación, extensión del motor. |
| [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) | Mediciones, metodología y cómo reproducirlas. |
| [`docs/AFIRMACIONES.md`](docs/AFIRMACIONES.md) | Qué se puede afirmar de HiveDB (y cómo decirlo): comparación con el stack habitual, afirmaciones verificadas y texto propuesto. |
| [`docs/AGENT_GUIDE.md`](docs/AGENT_GUIDE.md) | **Cómo usar HiveDB bien en un agente**: qué va en cada sitio, memoria semántica, recetas, operación y plan de adopción por proyecto. |
| [`docs/AGENT_INTEGRATION.md`](docs/AGENT_INTEGRATION.md) | Contrato de eventos para integrar un runtime de agentes con el harness causal. |
| [`docs/DISTRIBUTION.md`](docs/DISTRIBUTION.md) | Cómo se construyen y publican los binarios multiplataforma. |
| [`CHANGELOG.md`](CHANGELOG.md) | Cambios por versión: mejoras de rendimiento, formato en disco y migración. |
| [`docs/GATES.md`](docs/GATES.md) | Historial de hitos de la construcción del motor. |

## Desarrollo

Requisitos: Rust **1.96** o superior (`cargo`, `rustfmt`, `clippy`) y, para el paquete TypeScript, Bun.

```bash
cargo build --workspace --release

# Antes de entregar un cambio
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Concurrencia con el model checker loom
RUSTFLAGS="--cfg loom" cargo test --test g7_concurrency no_data_race_on_seq_assignment

# Paquete TypeScript
cd packages/hive-db
bun run build:native   # compila el binding y genera native.cjs
bun test
```

El workspace tiene cinco crates: `hivedb-core` (log, proyecciones, reactividad, consentimiento,
harness), `hivedb-index` (memoria semántica híbrida), `hivedb-embed` (embedder local opcional),
`hivedb-napi` (binding para Bun/Node) y `hivedb-bench` (benchmarks reproducibles). Cada hito nuevo
añade su archivo de tests `gN_*.rs`, registrado en el `Cargo.toml` del crate. Todo el tiempo pasa
por `Clock`; no se usa `SystemTime::now()` en la lógica del motor.

## Licencia

[Apache-2.0](LICENSE).
