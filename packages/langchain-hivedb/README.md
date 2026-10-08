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

## Varios adaptadores en una misma base

Cada adaptador separa lo suyo con un ámbito (`collection_name`), así que `HiveDBStore`,
`HiveDBVectorStore` y el historial pueden compartir la base sin mezclarse.

## Lo que no incluye (todavía)

El **checkpointer** de LangGraph (`BaseCheckpointSaver`) no está implementado: su contrato (blobs por
versión, historial de canales delta, `prune`, `copy_thread`…) pide una suite de conformidad que aún no
hemos pasado. Mientras tanto usa `InMemorySaver`/`SqliteSaver` para el estado del grafo y `HiveDBStore`
para la memoria que debe sobrevivir entre conversaciones.
