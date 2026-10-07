# HiveDB

Motor de base de datos embebido, local-first y agent-native — pensado para ser el motor de
memoria/persistencia de cualquier runtime de agentes; hoy usado por el ecosistema Hive.

HiveDB modela el estado como un **event-log append-only inmutable** sobre el que se derivan proyecciones deterministas. Está diseñado para correr in-process (primero en Bun vía napi-rs, luego nativo Rust), sin daemon ni dependencias de red.

## Documentación

| Archivo | Contenido |
|---|---|
| `SPEC.md` | Especificación del motor, principios de diseño y arquitectura de capas. |
| `TDD.md` | Contratos de test por fase (roadmap G1-G11). |
| `docs/USER_GUIDE.md` | Manual de uso desde Bun/TypeScript. |
| `docs/IMPLEMENTATION.md` | Manual de implementación y extensión del motor. |
| `docs/AGENT_INTEGRATION.md` | Contrato de eventos para integrar un runtime de agentes con el harness de larga duración (G9) — hiveCode es el primer consumidor de referencia, no el único soportado. |
| `docs/DISTRIBUTION.md` | Cómo consumir el paquete y publicarlo en npm con binarios para todos los SO. |
| `.kimi/plans/` | Planes de implementación aprobados por gate. |
| `packages/hive-db` | Capa TypeScript consumida por aplicaciones Bun. |

## Arquitectura

```
┌─────────────────────────────────────────────────────────┐
│  Capa TS (@johpaz/hive-db)  — API ergonómica para Bun   │
│  open(), append(), query(), subscribe(), project()      │
└───────────────────────────┬─────────────────────────────┘
                            │ napi-rs (C ABI)
┌───────────────────────────┴─────────────────────────────┐
│  Núcleo Rust (hivedb-core)                              │
│  ┌────────────┐  ┌─────────────┐  ┌──────────────────┐ │
│  │ Event Log  │  │ Projections │  │ Reactive Engine  │ │
│  │ (append)   │→ │ (state)     │← │ (triggers/subs)  │ │
│  └─────┬──────┘  └──────┬──────┘  └────────┬─────────┘ │
│        │                │                  │            │
│  ┌─────┴────────────────┴──────────────────┴─────────┐ │
│  │              Memory Tiers                          │ │
│  │  Working (RAM/TTL) · Episodic (log) · Semantic     │ │
│  └────────────────────────────────────────────────────┘ │
│        │              │                  │              │
│  ┌─────┴──────┐ ┌─────┴──────┐  ┌────────┴─────────┐   │
│  │ redb (KV)  │ │ tantivy    │  │ HNSW (vector)    │   │
│  │ log+state  │ │ (BM25/FTS) │  │ (ANN/semantic)   │   │
│  └────────────┘ └────────────┘  └──────────────────┘   │
│  ┌──────────────────────────────────────────────────┐  │
│  │ Consent Graph (delegación / intent audit)        │  │
│  └──────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

## Comparación con otros motores

HiveDB es un motor embebido de memoria para agentes: log de eventos inmutable, proyecciones,
motor reactivo, grafo de consentimiento y búsqueda híbrida (BM25 + vector + RRF). Sus rivales
reales son las capas de memoria para agentes y los motores vectoriales embebidos.

**Medido** (100 000 documentos, dim 384, k=10, datos sintéticos; detalle y límites en
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)):

| | Vector p50 | recall@10 | Arranque en frío | Disco | Búsqueda híbrida |
|---|---:|---:|---:|---:|:---:|
| **HiveDB** (HNSW) | 0,6 ms | 1,00 | 34 ms | 429 MiB | Sí (1,5 ms p50) |
| sqlite-vec (exacto) | 65,5 ms | 1,00 | 67 ms | 149 MiB | No |
| LanceDB (exacto) | 162,2 ms | 1,00 | 190 ms | 147 MiB | No |
| LanceDB (IVF_HNSW_SQ, ajustado) | 2,4 ms | 0,95 | 79 ms | 203 MiB | No |

A igual recall, la búsqueda vectorial de HiveDB es más rápida que la de LanceDB ajustado
(0,975 de recall en 0,5 ms frente a 0,95 en 2,4 ms). Honestidad por delante: HiveDB **ocupa
más disco (~2,1× LanceDB y ~2,9× sqlite-vec) e ingiere más despacio (~8k docs/s frente a
12k–109k)**. A cambio ofrece el log causal, el consentimiento y la búsqueda híbrida que los otros
no tienen. Detalle, curvas de `ef` y mejoras pendientes en `docs/BENCHMARKS.md`.

**Comparación cualitativa** (no medida; según la documentación pública de cada proyecto):

| | Enfoque | Dependencias típicas |
|---|---|---|
| **HiveDB** | Motor embebido: log causal, estado, consentimiento, búsqueda híbrida | Ninguna (un proceso); embedder local opcional |
| **Mem0** | Capa que extrae recuerdos con un LLM | LLM + almacén vectorial externo |
| **Zep / Graphiti** | Grafo de conocimiento temporal | Servicio + base de grafos + LLM |
| **Letta** | Framework de agentes con memoria por bloques | Servidor + base de datos |
| **LanceDB / sqlite-vec** | Almacén vectorial embebido | Ninguna; sin log causal ni consentimiento |
| **Turso (vectores)** | SQLite/libSQL con tipo vectorial | Pendiente de medir |

## Crates

- **`hivedb-core`** — motor de event-log, proyecciones, memoria de trabajo, motor reactivo y grafo de consentimiento.
- **`hivedb-index`** — índice semántico híbrido: BM25 (`tantivy`) + ANN (HNSW propio) + RRF propio.
- **`hivedb-embed`** — embedder local opcional (`multilingual-e5-small` sobre `candle`); ver `docs/USER_GUIDE.md`.
- **`hivedb-bench`** — benchmarks reproducibles ([`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)).
- **`hivedb-napi`** — binding napi-rs que expone `HiveDB` a Bun/Node.
- **`packages/hive-db`** — envoltorio TypeScript (`@johpaz/hive-db`) con tipos y async iterators.

## Estado actual

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

## Requisitos

- Rust **1.96.0** o superior (ver `rust-toolchain` si existe).
- `cargo`, `rustfmt`, `clippy`.
- Para tests de loom: `loom` se resuelve automáticamente via `cargo`.

## Construcción

```bash
cargo build --workspace --release
```

## Tests

### Rust

```bash
# Suite completa (incluye G1-G7)
cargo test --workspace

# Formato y linting
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Test de concurrencia con loom (model checker)
RUSTFLAGS="--cfg loom" cargo test --test g7_concurrency no_data_race_on_seq_assignment
```

### TypeScript / Bun (G8)

```bash
cd packages/hive-db
bun run build:native   # napi build --platform --release + renombra index.js -> native.cjs
bun test               # §4.11 - §4.11d
```

## CI

El workflow `.github/workflows/ci.yml` ejecuta:

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. En un job separado: `bun run build:napi` + `bun test`

## Principios de diseño (no negociables)

1. **Soberanía digital:** cero dependencia de servicios externos.
2. **Event-log como fuente de verdad:** todo estado es una proyección derivada; el log es append-only e inmutable.
3. **Embebido in-process:** corre dentro del proceso consumidor.
4. **Agent-native en el motor:** primitivos del agente viven en Rust, no se simulan arriba.
5. **`unsafe` minimizado:** solo en fronteras mmap/FFI.
6. **Determinismo y replay:** el estado debe poder reconstruirse reproduciendo el log desde cero.

## Contribuir

Cada cambio debe mantener verdes:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

Para gates nuevos, seguir el flujo de planificación en `.kimi/plans/` y actualizar este README con el estado.

## Licencia objetivo

Apache-2.0
