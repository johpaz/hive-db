import asyncio
import threading
import time

import pytest
from hivedb import AsyncHiveDB, HiveDBError


def test_suscripcion_recibe_eventos_coincidentes(db):
    with db.events({"kind": "ToolCall"}) as stream:
        db.append("a", "s", "Fact", {})
        db.append("a", "s", "ToolCall", {"tool": "grep"})
        event = stream.next(timeout=5)
        assert event is not None and event.kind_tag == "ToolCall"
        with pytest.raises(TimeoutError):
            stream.next(timeout=0.2)


def test_predicado_eq(db):
    pattern = {"predicate": {"kind": "Eq", "path": "/ok", "value": True}}
    with db.events(pattern) as stream:
        db.append("a", "s", "Fact", {"ok": False})
        db.append("a", "s", "Fact", {"ok": True})
        event = stream.next(timeout=5)
        assert event is not None and event.payload == {"ok": True}


def test_patron_invalido(db):
    with pytest.raises(HiveDBError, match="unknown kind"):
        db.events({"kind": "Otro"})


def test_close_termina_el_iterador(db):
    stream = db.events()
    threading.Timer(0.3, stream.close).start()
    started = time.time()
    assert list(stream) == []
    assert time.time() - started < 5


def test_callback_de_subscribe(db):
    received = []
    done = threading.Event()

    def on_event(event):
        received.append(event.kind_tag)
        done.set()

    stream = db.subscribe({"kind": "Fact"}, on_event)
    db.append("a", "s", "Fact", {})
    assert done.wait(5)
    stream.close()
    assert received == ["Fact"]


def test_lecturas_concurrentes_en_hilos(db):
    db.upsert_batch(
        [{"id": f"d{i}", "body": f"documento numero {i} sobre paella"} for i in range(200)]
    )
    errors = []

    def worker():
        try:
            for _ in range(50):
                assert db.query_hybrid(text="paella", k=5)
        except Exception as error:  # pragma: no cover
            errors.append(error)

    threads = [threading.Thread(target=worker) for _ in range(8)]
    [t.start() for t in threads]
    [t.join() for t in threads]
    assert errors == []


def test_cerrar_mientras_hay_lecturas(db):
    db.upsert_doc("d", body="algo")
    outcomes = []

    def reader():
        for _ in range(200):
            try:
                db.query_hybrid(text="algo")
            except HiveDBError as error:
                outcomes.append(str(error))
                return

    thread = threading.Thread(target=reader)
    thread.start()
    time.sleep(0.01)
    db.close()
    thread.join()
    assert all("closed" in o for o in outcomes)


def test_asyncio():
    async def main():
        async with await AsyncHiveDB.open(":memory:") as db:
            await db.append("a", "s", "Fact", {"x": 1})
            await db.upsert_doc("d", body="memoria asincrona")
            hits = await asyncio.gather(*[db.query_hybrid(text="memoria") for _ in range(8)])
            assert all(h[0].id == "d" for h in hits)
            notas = db.collection("n")
            await notas.put("1", {"v": 1})
            assert (await notas.get("1")).doc == {"v": 1}
            async with db.events({"kind": "Fact"}) as stream:
                await db.append("a", "s", "Fact", {"y": 2})
                async for event in stream:
                    assert event.payload == {"y": 2}
                    break
            with pytest.raises(HiveDBError):
                await db.append("a", "s", "ToolCall", {})

    asyncio.run(main())
