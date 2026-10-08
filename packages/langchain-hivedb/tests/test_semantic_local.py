"""Búsqueda por significado con el embedder local y un agente LangGraph con memoria persistente.
Requiere HIVEDB_E2E_EMBEDDER=1 (y HIVEDB_MODEL_DIR con el modelo o red para descargarlo)."""

import os

import pytest
from hivedb import HiveDB

from langchain_hivedb import HiveDBStore, HiveDBVectorStore

pytestmark = pytest.mark.skipif(
    os.environ.get("HIVEDB_E2E_EMBEDDER") != "1", reason="HIVEDB_E2E_EMBEDDER=1"
)


def test_vectorstore_con_embedder_local():
    with HiveDB.open(":memory:", embedder="local") as db:
        store = HiveDBVectorStore(db)
        store.add_texts(
            [
                "La paella es un plato de arroz típico de Valencia",
                "Cómo configurar el router de casa",
                "El equipo ganó el partido de fútbol por tres goles",
            ],
            metadatas=[{"tema": "cocina"}, {"tema": "tecnologia"}, {"tema": "deporte"}],
        )
        assert "paella" in store.similarity_search("cómo cocinar arroz", k=1)[0].page_content
        assert "fútbol" in store.similarity_search("who won the soccer match", k=1)[0].page_content
        filtered = store.similarity_search("arroz", k=3, filter={"tema": "deporte"})
        assert [d.metadata["tema"] for d in filtered] == ["deporte"]


def test_store_semantico_con_embedder_local():
    with HiveDB.open(":memory:", embedder="local") as db:
        store = HiveDBStore(db)
        store.put(("memorias", "ana"), "1", {"texto": "A Ana le encanta el arroz con mariscos"})
        store.put(("memorias", "ana"), "2", {"texto": "Ana trabaja como contadora de facturas"})
        hits = store.search(("memorias", "ana"), query="¿qué comida le gusta?", limit=1)
        assert [h.key for h in hits] == ["1"]


def test_agente_langgraph_recuerda_tras_reabrir(tmp_path):
    from langgraph.graph import END, START, StateGraph
    from langgraph.store.base import BaseStore
    from typing_extensions import TypedDict

    class State(TypedDict):
        mensaje: str
        respuesta: str

    def recordar(state: State, *, store: BaseStore):
        user = ("memorias", "ana")
        if state["mensaje"].startswith("recuerda:"):
            store.put(user, state["mensaje"], {"texto": state["mensaje"].split(":", 1)[1].strip()})
            return {"respuesta": "guardado"}
        hits = store.search(user, query=state["mensaje"], limit=1)
        return {"respuesta": hits[0].value["texto"] if hits else "no sé"}

    def construir(db):
        graph = StateGraph(State)
        graph.add_node("recordar", recordar)
        graph.add_edge(START, "recordar")
        graph.add_edge("recordar", END)
        return graph.compile(store=HiveDBStore(db))

    path = str(tmp_path / "agente")
    with HiveDB.open(path, embedder="local") as db:
        agente = construir(db)
        agente.invoke({"mensaje": "recuerda: mi cumpleaños es en marzo"})
        agente.invoke({"mensaje": "recuerda: prefiero el café sin azúcar"})
    with HiveDB.open(path, embedder="local") as db:
        respuesta = construir(db).invoke({"mensaje": "¿cuándo es mi cumpleaños?"})["respuesta"]
        assert "marzo" in respuesta
