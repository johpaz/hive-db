# johpaz-langchain-hivedb

Adaptadores de **LangChain** y **LangGraph** para [HiveDB](https://github.com/johpaz/hive-db), la base
de memoria embebida para agentes (motor en Rust, sin servidor).

```bash
pip install johpaz-langchain-hivedb       # VectorStore + historial de chat
pip install "johpaz-langchain-hivedb[langgraph]" # + HiveDBStore (BaseStore de LangGraph)
```

Todo se apoya en una base de HiveDB abierta con `johpaz-hive-db`. Lo más cómodo es el **embedder local**: HiveDB
genera los embeddings dentro del motor (multilingüe, sin llamar a ninguna API). El modelo (~470 MB) se
descarga **una vez por aplicación** al activarlo y lo comparten todas las bases del proceso:

```python
from hivedb import HiveDB

HiveDB.prepare_embedder(print)  # opcional: ver el avance de la primera descarga
db = HiveDB.open("./memoria", embedder="local")
```

## Memoria a largo plazo en LangGraph — `HiveDBStore`

```python
from langchain_hivedb import HiveDBStore

graph = builder.compile(store=HiveDBStore(db))

# dentro de un nodo:
store.put(("memorias", user_id), "gusto-1", {"texto": "le encanta el arroz con mariscos"})
store.search(("memorias", user_id), query="¿qué comida le gusta?", limit=3)  # por significado
```

* `search(..., query=...)` es **semántica + texto** (BM25 y significado, fusión RRF); sin `query` recorre
  el namespace en orden de clave. `filter` admite igualdad y `$eq/$ne/$gt/$gte/$lt/$lte`.
* `put(..., index=False)` excluye un elemento de la búsqueda; `index=["campo.sub"]` indexa solo esos campos.
* Persistente y compartible entre procesos; `list_namespaces` con `prefix`/`suffix`/`max_depth`.
* No admite TTL. `list_namespaces` y los `filter` recorren el namespace: para almacenes muy grandes usa
  un namespace por tema.

## Documentos — `HiveDBVectorStore`

```python
from langchain_hivedb import HiveDBVectorStore

store = HiveDBVectorStore(db)  # embeddings locales de HiveDB
store.add_texts(["La paella es un plato de arroz"], metadatas=[{"tema": "cocina"}])
store.similarity_search("cómo cocinar arroz", k=3, filter={"tema": "cocina"})
retriever = store.as_retriever(search_kwargs={"k": 4})
```

Con `HiveDBVectorStore(db, embedding=MisEmbeddings())` los vectores los calcula LangChain (abre la base con
`vector={"dimension": N, "space_id": "..."}`). Por defecto la búsqueda es híbrida; `hybrid=False` usa solo
significado. Las puntuaciones son «mayor = más parecido». Supera la suite de contrato `langchain-tests`.

## Historial de chat — `HiveDBChatMessageHistory`

```python
from langchain_hivedb import HiveDBChatMessageHistory

history = HiveDBChatMessageHistory(db, session_id="ana")
history.add_user_message("hola")
```

## Una acción por contexto — estado de tarea

Si el agente reinicia su contexto en cada paso, el grafo no necesita acumular mensajes: el estado de la
tarea (objetivo, hechos, callejones sin salida, plan…) vive en HiveDB y cada nodo lo lee, hace **una**
acción con un prompt corto y lo vuelve a escribir. Con `expected_version`, otra instancia no se pisa en
silencio. (Requiere la versión de `johpaz-hive-db` que incluye `save_task_state`.)

```python
from typing import TypedDict
from hivedb import HiveDB, render_task_state
from langgraph.graph import END, START, StateGraph

db = HiveDB.open("./memoria")
AGENTE, TAREA = "worker", "migracion"
db.save_task_state(AGENTE, TAREA, {"objective": "Migrar la facturación", "plan": ["leer API", "mapear", "probar"]})


class Estado(TypedDict):
    hecho: bool  # el grafo casi no lleva nada: la memoria vive en HiveDB


def paso(_: Estado) -> Estado:
    estado = db.load_task_state(AGENTE, TAREA)                 # 1. leer el estado
    if not estado.plan:
        return {"hecho": True}
    siguiente, *resto = estado.plan
    # 2. una acción; el modelo recibe solo render_task_state(estado), no un historial
    resultado = hacer(siguiente, contexto=render_task_state(estado))
    db.save_task_state(                                        # 3. escribir el estado nuevo
        AGENTE, TAREA,
        {**vars(estado), "changes": estado.changes + [resultado], "plan": resto},
        expected_version=estado.version,
    )
    return {"hecho": not resto}


g = StateGraph(Estado)
g.add_node("paso", paso)
g.add_edge(START, "paso")
g.add_conditional_edges("paso", lambda s: END if s["hecho"] else "paso")
g.compile().invoke({"hecho": False})
```

## Varios adaptadores en una misma base

Cada adaptador separa lo suyo con un ámbito (`collection_name`), así que `HiveDBStore`,
`HiveDBVectorStore` y el historial pueden compartir la base sin mezclarse.

## Lo que no incluye (todavía)

El **checkpointer** de LangGraph (`BaseCheckpointSaver`) no está implementado: su contrato (blobs por
versión, historial de canales delta, `prune`, `copy_thread`…) pide una suite de conformidad que aún no
hemos pasado. Mientras tanto usa `InMemorySaver`/`SqliteSaver` para el estado del grafo y `HiveDBStore`
para la memoria que debe sobrevivir entre conversaciones.
