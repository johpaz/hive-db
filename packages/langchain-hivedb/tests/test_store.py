import asyncio

import pytest
from hivedb import HiveDB
from langgraph.store.base import GetOp, ListNamespacesOp, MatchCondition, PutOp, SearchOp

from langchain_hivedb import HiveDBStore


@pytest.fixture
def store():
    db = HiveDB.open(":memory:")
    yield HiveDBStore(db)
    db.close()


def test_put_get_delete(store):
    store.put(("usuarios", "ana"), "perfil", {"idioma": "es", "edad": 30})
    item = store.get(("usuarios", "ana"), "perfil")
    assert item is not None and item.value == {"idioma": "es", "edad": 30}
    assert item.key == "perfil" and item.namespace == ("usuarios", "ana")
    created = item.created_at
    store.put(("usuarios", "ana"), "perfil", {"idioma": "en"})
    updated = store.get(("usuarios", "ana"), "perfil")
    assert updated.value == {"idioma": "en"} and updated.created_at == created
    assert updated.updated_at >= created
    store.delete(("usuarios", "ana"), "perfil")
    assert store.get(("usuarios", "ana"), "perfil") is None


def test_namespace_no_se_confunde_con_prefijo_parcial(store):
    store.put(("a", "b"), "k", {"v": 1})
    store.put(("a", "bc"), "k", {"v": 2})
    store.put(("a", "b", "c"), "k", {"v": 3})
    values = sorted(i.value["v"] for i in store.search(("a", "b")))
    assert values == [1, 3]


def test_busqueda_con_filtros_y_paginacion(store):
    for i in range(5):
        store.put(("n",), f"k{i}", {"i": i, "tipo": "par" if i % 2 == 0 else "impar"})
    assert [x.key for x in store.search(("n",), filter={"tipo": "par"})] == ["k0", "k2", "k4"]
    assert [x.key for x in store.search(("n",), filter={"i": {"$gte": 3}})] == ["k3", "k4"]
    assert [x.key for x in store.search(("n",), limit=2, offset=1)] == ["k1", "k2"]


def test_busqueda_por_texto_en_el_namespace(store):
    store.put(("m", "ana"), "1", {"texto": "le gusta la paella valenciana"})
    store.put(("m", "ana"), "2", {"texto": "trabaja con facturas electrónicas"})
    store.put(("m", "luis"), "1", {"texto": "paella con mariscos"})
    hits = store.search(("m", "ana"), query="paella")
    assert [h.key for h in hits] == ["1"] and hits[0].namespace == ("m", "ana")
    assert hits[0].score is not None
    assert {h.namespace for h in store.search(("m",), query="paella")} == {
        ("m", "ana"),
        ("m", "luis"),
    }


def test_index_false_excluye_de_la_busqueda_y_campos_acotan(store):
    store.put(("x",), "oculto", {"texto": "secreto de paella"}, index=False)
    store.put(("x",), "campos", {"titulo": "paella", "nota": "irrelevante"}, index=["titulo"])
    assert [h.key for h in store.search(("x",), query="paella")] == ["campos"]
    assert store.search(("x",), query="irrelevante") == []
    store.put(("x",), "campos", {"titulo": "otro"}, index=["titulo"])
    assert store.search(("x",), query="paella") == []


def test_borrar_quita_del_indice(store):
    store.put(("x",), "a", {"texto": "memoria temporal"})
    store.delete(("x",), "a")
    assert store.search(("x",), query="memoria") == []


def test_list_namespaces(store):
    for ns in [("a", "b", "c"), ("a", "b", "d"), ("a", "e"), ("f",)]:
        store.put(ns, "k", {"v": 1})
    assert store.list_namespaces() == [("a", "b", "c"), ("a", "b", "d"), ("a", "e"), ("f",)]
    assert store.list_namespaces(prefix=("a", "b")) == [("a", "b", "c"), ("a", "b", "d")]
    assert store.list_namespaces(suffix=("e",)) == [("a", "e")]
    assert store.list_namespaces(prefix=("a", "*", "c")) == [("a", "b", "c")]
    assert store.list_namespaces(max_depth=2) == [("a", "b"), ("a", "e"), ("f",)]
    assert store.list_namespaces(limit=1, offset=1) == [("a", "b", "d")]


def test_batch_mezclado(store):
    results = store.batch(
        [
            PutOp(("n",), "k", {"v": 1}),
            GetOp(("n",), "k"),
            SearchOp(("n",), None, limit=10, offset=0),
            ListNamespacesOp(
                match_conditions=(MatchCondition("prefix", ("n",)),),
                max_depth=None,
                limit=10,
                offset=0,
            ),
        ]
    )
    assert results[0] is None
    assert results[1].value == {"v": 1}
    assert [i.key for i in results[2]] == ["k"]
    assert results[3] == [("n",)]


def test_partes_invalidas(store):
    with pytest.raises(ValueError):
        store.put(("a\x1fb",), "k", {"v": 1})
    with pytest.raises(ValueError):
        store.put(("a",), "", {"v": 1})


def test_async(store):
    async def main():
        await store.aput(("n",), "k", {"texto": "asincrono"})
        assert (await store.aget(("n",), "k")).value == {"texto": "asincrono"}
        assert [h.key for h in await store.asearch(("n",), query="asincrono")] == ["k"]

    asyncio.run(main())


def test_persiste_al_reabrir(tmp_path):
    path = str(tmp_path / "s")
    with HiveDB.open(path) as db:
        HiveDBStore(db).put(("p",), "k", {"texto": "persistente"})
    with HiveDB.open(path) as db:
        s = HiveDBStore(db)
        assert s.get(("p",), "k").value == {"texto": "persistente"}
        assert [h.key for h in s.search(("p",), query="persistente")] == ["k"]


def test_dos_almacenes_en_la_misma_base_no_se_mezclan():
    from langchain_hivedb import HiveDBVectorStore

    with HiveDB.open(":memory:") as db:
        store = HiveDBStore(db)
        docs = HiveDBVectorStore(db)
        store.put(("n",), "k", {"texto": "paella"})
        docs.add_texts(["paella en el vector store"], ids=["d1"])
        assert [h.key for h in store.search(("n",), query="paella")] == ["k"]
        assert [d.id for d in docs.similarity_search("paella")] == ["d1"]
