import pytest
from hivedb import FieldBoosts, Fusion, HiveDB, HiveDBError, IndexDoc, VectorOptions, error_code


def test_texto_con_filtros_y_borrado(db):
    db.upsert_batch(
        [
            IndexDoc("a", body="receta de paella valenciana", filters={"tenant": "x"}),
            IndexDoc("b", body="receta de paella con mariscos", filters={"tenant": "y"}),
            {"id": "c", "name": "Paella", "body": "arroz"},
        ]
    )
    assert {h.id for h in db.query_hybrid(text="paella", k=10)} == {"a", "b", "c"}
    assert [h.id for h in db.query_hybrid(text="paella", filters={"tenant": "x"})] == ["a"]
    assert [h.id for h in db.query_hybrid(text="paella", filters=[("tenant", "y")])] == ["b"]
    db.delete_by_filter("tenant", "x")
    assert "a" not in {h.id for h in db.query_hybrid(text="paella", k=10)}
    db.delete_doc("b")
    db.clear_index()
    assert db.query_hybrid(text="paella") == []


def test_las_interrogativas_con_acento_no_cuentan(db):
    db.upsert_doc("d", body="Cómo configurar el router de casa")
    assert db.query_hybrid(text="¿cómo cocinar arroz?") == []


def test_palabras_vacias_solas_no_encuentran_nada(db):
    db.upsert_doc("d", body="la casa de la pradera")
    assert db.query_hybrid(text="de la") == []


def test_fusion_y_pesos_validos(vdb):
    vdb.upsert_doc("a", body="zorro marrón", vector=[1, 0, 0, 0])
    vdb.upsert_doc("b", body="perro perezoso", vector=[0, 1, 0, 0])
    hits = vdb.query_hybrid(
        text="zorro",
        vector=[1, 0, 0, 0],
        k=2,
        fusion=Fusion("rrf", 60),
        boosts=FieldBoosts(body=3.0),
    )
    assert hits[0].id == "a"
    assert hits[0].text_score is not None and hits[0].vector_score is not None
    with pytest.raises(HiveDBError, match="unknown fusion kind"):
        vdb.query_hybrid(text="zorro", fusion="otra")


def test_vectores_aceptan_array_y_tuplas(vdb):
    import array

    vdb.upsert_doc("a", vector=array.array("f", [1, 0, 0, 0]))
    vdb.upsert_doc("b", vector=(0.0, 1.0, 0.0, 0.0))
    hits = vdb.query_hybrid(vector=(1, 0, 0, 0), k=1)
    assert [h.id for h in hits] == ["a"]
    assert hits[0].vector_score == pytest.approx(1.0)


def test_dimension_incorrecta_lleva_codigo(vdb):
    with pytest.raises(HiveDBError) as caught:
        vdb.upsert_doc("a", vector=[1, 0])
    assert caught.value.code == "INVALID_VECTOR"
    assert error_code(caught.value) == "INVALID_VECTOR"


def test_sin_configuracion_vectorial_el_vector_falla_con_codigo(db):
    with pytest.raises(HiveDBError) as caught:
        db.upsert_doc("a", vector=[1, 0, 0, 0])
    assert caught.value.code == "INVALID_VECTOR"


def test_espacio_vectorial_distinto_al_reabrir(tmp_path):
    path = str(tmp_path / "v")
    with HiveDB.open(path, vector=VectorOptions(4, "modelo-a")) as db:
        db.upsert_doc("a", vector=[1, 0, 0, 0])
    with pytest.raises(HiveDBError) as caught:
        HiveDB.open(path, vector={"dimension": 4, "space_id": "modelo-b"})
    assert caught.value.code == "VECTOR_SPACE_MISMATCH"


def test_traza_del_hnsw(vdb):
    for i in range(4):
        vector = [0.0] * 4
        vector[i] = 1.0
        vdb.upsert_doc(f"d{i}", vector=vector)
    trace = vdb.trace_vector([1, 0, 0, 0], k=2)
    assert trace.hits[0].id == "d0"
    assert trace.steps and trace.steps[0].from_ is None


def test_compactar_conserva_los_resultados(vdb):
    vdb.upsert_doc("a", body="uno", vector=[1, 0, 0, 0])
    antes = vdb.query_hybrid(text="uno", vector=[1, 0, 0, 0])
    vdb.compact_index()
    assert [h.id for h in vdb.query_hybrid(text="uno", vector=[1, 0, 0, 0])] == [
        h.id for h in antes
    ]
