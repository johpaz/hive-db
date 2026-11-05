"""Estado de tarea: el traspaso que un agente deja a su siguiente instancia.

Un agente que reinicia su contexto con frecuencia (compactación, o una sola acción por contexto)
necesita un estado corto, explícito y legible que viaje entre instancias. Seis campos, los de
«You don't need long context» (arXiv 2611.05306): objetivo, hechos establecidos, cambios aplicados,
verificación, callejones sin salida y plan restante.

Es una colección de documentos como cualquier otra (duradera y con versión optimista), no memoria
de trabajo: la memoria de trabajo caduca y un estado de tarea debe sobrevivir a un reinicio. El
documento guardado es idéntico al del paquete npm (`deadEnds` en camelCase), así que un estado
guardado desde Python se lee desde Node y al revés. Este módulo es puro (sin motor).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Mapping, Optional, Union
from urllib.parse import quote

TASK_STATE_COLLECTION = "task_states"

_MAX_ITEMS = 30
_MAX_ITEM_CHARS = 500
_MAX_OBJECTIVE_CHARS = 400


@dataclass(frozen=True)
class TaskState:
    #: Una línea: el resultado que se busca.
    objective: str = ""
    #: Hechos verificados, exactos: rutas, ids, valores, errores, preferencias.
    facts: List[str] = field(default_factory=list)
    #: Qué se cambió o creó, y si se verificó.
    changes: List[str] = field(default_factory=list)
    #: Comprobaciones hechas y su resultado.
    verification: List[str] = field(default_factory=list)
    #: Enfoques que fallaron y no deben repetirse.
    dead_ends: List[str] = field(default_factory=list)
    #: Subtareas pendientes en orden; la primera es el siguiente paso.
    plan: List[str] = field(default_factory=list)


@dataclass(frozen=True)
class StoredTaskState(TaskState):
    """Un estado guardado, con lo que el motor añade al persistirlo."""

    agent_id: str = ""
    task_id: str = ""
    #: Cuántas veces se ha guardado este estado (1 la primera).
    step: int = 0
    #: Milisegundos desde epoch del último guardado.
    updated_at: int = 0
    #: Versión de la colección: pásala como `expected_version` para no pisar a otra instancia.
    version: int = 0


TaskStateInput = Union[TaskState, Mapping[str, Any], None]


def task_state_id(agent_id: str, task_id: str) -> str:
    """Id de documento sin ambigüedad: dos agentes o tareas distintos nunca colisionan."""
    if not agent_id or not task_id:
        raise ValueError("task_state: agent_id y task_id son obligatorios")
    return f"{quote(agent_id, safe='')}:{quote(task_id, safe='')}"


def _clean(value: Any, limit: int) -> str:
    import json

    if isinstance(value, str):
        text = value
    elif value is None:
        text = ""
    else:
        text = json.dumps(value, ensure_ascii=False)
    return " ".join(text.split())[:limit]


def _clean_list(value: Any) -> List[str]:
    if isinstance(value, (list, tuple)):
        items: List[Any] = list(value)
    elif isinstance(value, str) and value.strip():
        items = [value]
    else:
        items = []
    return [t for t in (_clean(i, _MAX_ITEM_CHARS) for i in items) if t][:_MAX_ITEMS]


def normalize_task_state(state: TaskStateInput) -> TaskState:
    """Deja el estado en forma canónica: textos recortados, listas acotadas y sin vacíos.

    Acepta entradas parciales (lo que falte queda vacío) y un diccionario con `dead_ends` o
    `deadEnds`, para que un modelo que omite un campo no rompa el guardado."""
    if state is None:
        raw: Mapping[str, Any] = {}
    elif isinstance(state, TaskState):
        raw = {
            "objective": state.objective,
            "facts": state.facts,
            "changes": state.changes,
            "verification": state.verification,
            "dead_ends": state.dead_ends,
            "plan": state.plan,
        }
    else:
        raw = state
    dead_ends = raw.get("dead_ends", raw.get("deadEnds"))
    return TaskState(
        objective=_clean(raw.get("objective"), _MAX_OBJECTIVE_CHARS),
        facts=_clean_list(raw.get("facts")),
        changes=_clean_list(raw.get("changes")),
        verification=_clean_list(raw.get("verification")),
        dead_ends=_clean_list(dead_ends),
        plan=_clean_list(raw.get("plan")),
    )


def task_state_doc(
    state: TaskState, agent_id: str, task_id: str, step: int, updated_at: int
) -> Dict[str, Any]:
    """El documento tal como se guarda (mismas claves que el paquete npm)."""
    return {
        "objective": state.objective,
        "facts": list(state.facts),
        "changes": list(state.changes),
        "verification": list(state.verification),
        "deadEnds": list(state.dead_ends),
        "plan": list(state.plan),
        "agentId": agent_id,
        "taskId": task_id,
        "step": step,
        "updatedAt": updated_at,
    }


def stored_task_state(doc: Mapping[str, Any], version: int) -> StoredTaskState:
    base = normalize_task_state(doc)
    return StoredTaskState(
        objective=base.objective,
        facts=base.facts,
        changes=base.changes,
        verification=base.verification,
        dead_ends=base.dead_ends,
        plan=base.plan,
        agent_id=str(doc.get("agentId", "")),
        task_id=str(doc.get("taskId", "")),
        step=int(doc.get("step", 0)),
        updated_at=int(doc.get("updatedAt", 0)),
        version=version,
    )


def render_task_state(state: TaskState, max_chars: Optional[int] = None) -> str:
    """El estado como texto para un prompt, en orden de prioridad: quien recorte la cola pierde
    primero la verificación y los cambios, y último los hechos y los callejones sin salida."""

    def lista(titulo: str, items: List[str]) -> str:
        return f"{titulo}:\n" + "\n".join(f"- {i}" for i in items) if items else ""

    texto = "\n\n".join(
        parte
        for parte in (
            f"Objetivo: {state.objective}" if state.objective else "",
            lista("Hechos establecidos", state.facts),
            lista("Callejones sin salida (no repetir)", state.dead_ends),
            lista("Plan restante", state.plan),
            lista("Cambios aplicados", state.changes),
            lista("Verificación", state.verification),
        )
        if parte
    )
    return texto if max_chars is None or len(texto) <= max_chars else texto[: max(0, max_chars)]
