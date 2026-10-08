import pytest
from hivedb import HiveDB, HiveDBError


def test_append_y_lectura(db):
    seq = db.append("agente-1", "tarea-1", "Fact", {"temperatura": 21.5})
    assert seq == 1
    event = db.read(seq)
    assert (event.agent_id, event.stream_id, event.kind_tag) == ("agente-1", "tarea-1", "Fact")
    assert event.payload == {"temperatura": 21.5}
    assert db.log_len() == 1 and db.last_seq() == 1


def test_tipos_con_campos_obligatorios(db):
    with pytest.raises(HiveDBError, match="ToolCall requires payload.tool"):
        db.append("a", "s", "ToolCall", {})
    with pytest.raises(HiveDBError, match="unknown event kind: Inventado"):
        db.append("a", "s", "Inventado", {})
    db.append("a", "s", "ToolCall", {"tool": "grep"})
    stats = db.tool_stats("grep", agents=["a"])
    assert stats is not None and stats.invocations == 1


def test_correlacion_uuid_invalida(db):
    with pytest.raises(HiveDBError, match="invalid correlation UUID"):
        db.append("a", "s", "Fact", {}, correlation="no-es-uuid")
    seq = db.append("a", "s", "Fact", {}, correlation="123e4567-e89b-12d3-a456-426614174000")
    assert db.read(seq).correlation == "123e4567-e89b-12d3-a456-426614174000"


def test_memoria_de_trabajo(db):
    db.working_set("a", "k", {"n": [1, 2, 3]})
    assert db.working_get("a", "k") == {"n": [1, 2, 3]}
    assert db.working_keys("a") == ["k"]
    assert db.working_get("a", "otra") is None


def test_colecciones_y_versiones(db):
    notas = db.collection("notas")
    assert notas.put("1", {"titulo": "uno", "cliente": "acme"}, expected_version=0) == 1
    with pytest.raises(HiveDBError):
        notas.put("1", {"titulo": "otra"}, expected_version=0)
    assert notas.put("1", {"titulo": "dos", "cliente": "acme"}, expected_version=1) == 2
    entry = notas.get("1")
    assert entry is not None and entry.version == 2 and entry.doc["titulo"] == "dos"
    assert notas.get("no-existe") is None

    notas.put("2", {"titulo": "tres", "cliente": "otro"})
    assert notas.count() == 2
    notas.create_index("cliente")
    assert [e.id for e in notas.find_by("cliente", "acme")] == ["1"]
    assert [e.id for e in notas.scan({"reverse": True})] == ["2", "1"]
    assert [e.id for e in notas.scan({"limit": 1})] == ["1"]
    assert notas.delete("1") is True and notas.delete("1") is False


def test_indice_unico(db):
    usuarios = db.collection("usuarios")
    usuarios.create_index("email", unique=True)
    usuarios.put("1", {"email": "a@x.com"})
    with pytest.raises(HiveDBError):
        usuarios.put("2", {"email": "a@x.com"})


def test_batch_atomico(db):
    db.batch(
        [
            {"op": "put", "collection": "c", "id": "1", "doc": {"n": 1}},
            {"op": "put", "collection": "c", "id": "2", "doc": {"n": 2}},
        ]
    )
    assert db.collection("c").count() == 2
    with pytest.raises(HiveDBError):
        db.batch(
            [
                {"op": "put", "collection": "c", "id": "3", "doc": {"n": 3}},
                {
                    "op": "put",
                    "collection": "c",
                    "id": "1",
                    "doc": {"n": 9},
                    "expected_version": 99,
                },
            ]
        )
    assert db.collection("c").count() == 2  # la tanda fallida no dejó nada


def test_persistencia_al_reabrir(tmp_path):
    path = str(tmp_path / "memoria")
    with HiveDB.open(path) as db:
        db.append("a", "s", "Fact", {"v": 1})
        db.collection("c").put("1", {"v": 1})
        db.upsert_doc("d", body="persistente")
    with HiveDB.open(path) as db:
        assert db.log_len() == 1
        assert db.collection("c").get("1").doc == {"v": 1}
        assert [h.id for h in db.query_hybrid(text="persistente")] == ["d"]


def test_base_cerrada(db):
    db.close()
    db.close()  # idempotente
    with pytest.raises(HiveDBError, match="database is closed"):
        db.log_len()
