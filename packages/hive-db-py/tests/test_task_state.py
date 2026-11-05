import asyncio

import pytest
from hivedb import (
    AsyncHiveDB,
    HiveDB,
    HiveDBError,
    StoredTaskState,
    TaskState,
    normalize_task_state,
    render_task_state,
)

ESTADO = TaskState(
    objective="Migrar la facturación a Alegra",
    facts=["API base: https://api.alegra.com/v1", "El usuario prefiere respuestas cortas"],
    changes=["Creado src/billing/alegra.py (verificado)"],
    verification=["pytest billing: 12 passed"],
    dead_ends=["Basic auth falla con 401; usar token"],
    plan=["Mapear impuestos", "Probar nota crédito"],
)


def test_ida_y_vuelta_con_paso_version_y_hora(db):
    guardado = db.save_task_state("agente-a", "tarea-1", ESTADO)
    assert isinstance(guardado, StoredTaskState)
    assert (guardado.step, guardado.version) == (1, 1)
    assert guardado.updated_at > 0

    assert db.load_task_state("agente-a", "tarea-1") == guardado


def test_cada_guardado_reemplaza_al_anterior_y_sube_el_paso(db):
    db.save_task_state("agente-a", "tarea-1", ESTADO)
    segundo = db.save_task_state("agente-a", "tarea-1", {"objective": "Otro", "plan": ["Uno"]})

    assert (segundo.step, segundo.version) == (2, 2)
    assert segundo.facts == []
    assert db.load_task_state("agente-a", "tarea-1").objective == "Otro"


def test_expected_version_no_pisa_un_guardado_mas_nuevo(db):
    primero = db.save_task_state("agente-a", "tarea-1", ESTADO, expected_version=0)
    db.save_task_state("agente-a", "tarea-1", {"objective": "de otra instancia"})

    with pytest.raises(HiveDBError):
        db.save_task_state("agente-a", "tarea-1", ESTADO, expected_version=primero.version)
    assert db.load_task_state("agente-a", "tarea-1").objective == "de otra instancia"


def test_aislado_por_agente_y_tarea_aunque_el_id_lleve_dos_puntos(db):
    db.save_task_state("a:b", "c", {"objective": "uno"})
    db.save_task_state("a", "b:c", {"objective": "dos"})

    assert db.load_task_state("a:b", "c").objective == "uno"
    assert db.load_task_state("a", "b:c").objective == "dos"
    assert db.load_task_state("a", "c") is None


def test_borrar_informa_si_existia(db):
    db.save_task_state("agente-a", "tarea-1", ESTADO)
    assert db.delete_task_state("agente-a", "tarea-1") is True
    assert db.delete_task_state("agente-a", "tarea-1") is False
    assert db.load_task_state("agente-a", "tarea-1") is None


def test_no_escribe_en_el_registro_de_eventos(db):
    db.save_task_state("agente-a", "tarea-1", ESTADO)
    assert db.log_len() == 0


def test_agente_y_tarea_son_obligatorios(db):
    with pytest.raises(ValueError):
        db.save_task_state("", "tarea-1", ESTADO)
    with pytest.raises(ValueError):
        db.load_task_state("agente-a", "")


def test_el_documento_es_el_mismo_que_el_del_paquete_npm(db):
    db.save_task_state("agente-a", "tarea-1", ESTADO)
    doc = db.collection("task_states").scan()[0].doc
    assert doc["deadEnds"] == ESTADO.dead_ends
    assert {"agentId", "taskId", "step", "updatedAt"} <= set(doc)
    assert "dead_ends" not in doc


def test_persiste_al_reabrir(tmp_path):
    ruta = str(tmp_path / "db")
    with HiveDB.open(ruta) as db:
        db.save_task_state("agente-a", "tarea-1", ESTADO)
    with HiveDB.open(ruta) as db:
        assert db.load_task_state("agente-a", "tarea-1").dead_ends == ESTADO.dead_ends


def test_normalizar_recorta_descarta_vacios_y_tolera_parciales():
    n = normalize_task_state(
        {
            "objective": "  o  " * 300,
            "facts": ["  a   b  ", "", "x" * 2000] + [f"h{i}" for i in range(50)],
            "deadEnds": ["falló"],
        }
    )
    assert len(n.objective) <= 400
    assert n.facts[0] == "a b" and len(n.facts[1]) == 500 and len(n.facts) == 30
    assert n.dead_ends == ["falló"] and n.plan == []
    assert normalize_task_state(None).objective == ""


def test_render_ordena_por_prioridad_y_recorta_la_cola():
    texto = render_task_state(ESTADO)
    orden = [
        "Hechos establecidos",
        "Callejones sin salida",
        "Plan restante",
        "Cambios aplicados",
        "Verificación",
    ]
    posiciones = [texto.index(t) for t in orden]
    assert posiciones == sorted(posiciones)

    corto = render_task_state(ESTADO, max_chars=220)
    assert len(corto) <= 220 and "API base" in corto and "Callejones" in corto
    assert render_task_state(normalize_task_state({"objective": "x", "plan": ["a"]})) == (
        "Objetivo: x\n\nPlan restante:\n- a"
    )


def test_cliente_asincrono():
    async def go():
        async with await AsyncHiveDB.open(":memory:") as db:
            guardado = await db.save_task_state("agente-a", "tarea-1", ESTADO)
            assert await db.load_task_state("agente-a", "tarea-1") == guardado
            assert await db.delete_task_state("agente-a", "tarea-1") is True

    asyncio.run(go())
