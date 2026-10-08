# hive-db (Python)

Binding Python de **HiveDB**, la base de memoria embebida para agentes de IA: registro inmutable de
eventos, colecciones de documentos, búsqueda híbrida (BM25 + vectores con fusión RRF) y un embedder
local opcional. El motor es el mismo que el del paquete npm `@johpaz/hive-db` (Rust), sin servidor y
sin dependencias de Python.

```bash
pip install johpaz-hive-db
```

Wheels para Linux (glibc y musl, x64 y arm64), macOS (x64 y arm64) y Windows (x64); una sola wheel
por plataforma sirve a Python 3.9 en adelante.

## Uso rápido

```python
from hivedb import HiveDB

with HiveDB.open("./memoria") as db:
    # Registro de eventos (inmutable)
    seq = db.append("agente-1", "tarea-7", "Fact", {"temperatura": 21.5})

    # Colecciones con versión optimista
    notas = db.collection("notas")
    notas.put("n1", {"titulo": "Factura", "cliente": "acme"})
    notas.create_index("cliente")
    print(notas.find_by("cliente", "acme"))

    # Búsqueda de texto (BM25, español e inglés)
    db.upsert_doc("d1", body="El cliente prefiere factura electrónica", filters={"tenant": "acme"})
    print(db.query_hybrid(text="factura", k=5, filters={"tenant": "acme"}))
```

### Embeddings locales (búsqueda por significado)

```python
from hivedb import HiveDB

# Una sola vez por aplicación: descarga (~470 MB) y verifica el modelo, con progreso.
HiveDB.prepare_embedder(lambda p: print(f"{p.file}: {p.downloaded}/{p.total}"))

db = HiveDB.open("./memoria", embedder="local")   # no hace falta indicar `vector`
db.upsert_doc("d2", body="Receta de paella valenciana")
print(db.query_hybrid(text="cómo cocinar arroz", k=3))
```

El modelo **no viaja en el paquete**: se descarga al activarlo y se comparte entre todas las bases
del proceso (un modelo por aplicación, ~12 MiB por base adicional). Variables: `HIVEDB_MODEL_DIR`
(caché), `HIVEDB_MODEL_BASE_URL` (espejo), `HIVEDB_OFFLINE=1` (sin red).

### asyncio

```python
from hivedb import AsyncHiveDB

async with await AsyncHiveDB.open("./memoria", embedder="local") as db:
    hits = await db.query_hybrid(text="factura", k=5)
    async for event in db.events({"kind": "Fact"}):
        ...
```

Las llamadas sueltan el GIL mientras trabaja el motor: varios hilos (o tareas con `AsyncHiveDB`)
consultan la misma base a la vez.

### Errores

Todos los errores del motor son `HiveDBError`; los semánticos llevan `code`
(`INVALID_VECTOR`, `VECTOR_SPACE_MISMATCH`, `INDEX_DEGRADED`, `EMBEDDER_UNAVAILABLE`):

```python
from hivedb import HiveDBError

try:
    HiveDB.open("./m", embedder="local")
except HiveDBError as error:
    if error.code == "EMBEDDER_UNAVAILABLE":
        ...
```

## Bases compartidas por varios inquilinos

Igual que en Node: usa `filters={"tenant": ...}` en `upsert_doc` y en `query_hybrid`, y pasa
`agents=[...]` a `tool_stats` y `causal_thread`. Para inquilinos grandes, una base por inquilino.
Guía completa en [`docs/AGENT_GUIDE.md`](../../docs/AGENT_GUIDE.md).

## Licencia

Apache-2.0.
