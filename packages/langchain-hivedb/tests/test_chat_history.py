import pytest
from hivedb import HiveDB
from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

from langchain_hivedb import HiveDBChatMessageHistory


def test_mensajes_en_orden_y_aislados_por_sesion(tmp_path):
    with HiveDB.open(str(tmp_path / "h")) as db:
        a = HiveDBChatMessageHistory(db, "sesion-a")
        b = HiveDBChatMessageHistory(db, "sesion-b")
        a.add_messages([SystemMessage(content="eres útil"), HumanMessage(content="hola")])
        a.add_ai_message("¿en qué ayudo?")
        b.add_user_message("otra conversación")
        assert [type(m) for m in a.messages] == [SystemMessage, HumanMessage, AIMessage]
        assert [m.content for m in a.messages] == ["eres útil", "hola", "¿en qué ayudo?"]
        assert [m.content for m in b.messages] == ["otra conversación"]
        a.clear()
        assert a.messages == [] and len(b.messages) == 1


def test_persiste_al_reabrir(tmp_path):
    path = str(tmp_path / "h")
    with HiveDB.open(path) as db:
        HiveDBChatMessageHistory(db, "s").add_user_message("recuérdame")
    with HiveDB.open(path) as db:
        assert [m.content for m in HiveDBChatMessageHistory(db, "s").messages] == ["recuérdame"]


def test_session_id_invalido():
    with HiveDB.open(":memory:") as db, pytest.raises(ValueError):
        HiveDBChatMessageHistory(db, "a\x1fb")
