"""Conversión entre los tipos públicos y los `dict` del módulo nativo."""

from __future__ import annotations

from dataclasses import asdict, is_dataclass
from typing import Any, Dict, List, Mapping, Optional, Sequence

from .types import (
    Decision,
    DocEntry,
    Event,
    EventPattern,
    FieldBoosts,
    FiltersInput,
    Fusion,
    Hit,
    IndexDoc,
    ModelProgress,
    PreparedEmbedder,
    ScalarFilter,
    ScanOptions,
    ToolStats,
    TraceStep,
    VectorTrace,
)


def vector_in(vector: Optional[Sequence[float]]) -> Optional[List[float]]:
    """Lista de `float` a partir de una lista, `array.array`, tupla o array de numpy."""
    if vector is None:
        return None
    tolist = getattr(vector, "tolist", None)
    values = tolist() if callable(tolist) else vector
    return [float(x) for x in values]


def filters_in(filters: Optional[FiltersInput]) -> Optional[List[Dict[str, str]]]:
    if filters is None:
        return None
    if isinstance(filters, Mapping):
        if set(filters) == {"field", "value"}:
            return [{"field": str(filters["field"]), "value": str(filters["value"])}]
        return [{"field": str(k), "value": str(v)} for k, v in filters.items()]
    out: List[Dict[str, str]] = []
    for item in filters:
        if isinstance(item, ScalarFilter):
            out.append({"field": item.field, "value": item.value})
        elif isinstance(item, Mapping):
            out.append({"field": str(item["field"]), "value": str(item["value"])})
        else:
            field, value = item
            out.append({"field": str(field), "value": str(value)})
    return out


def _drop_none(data: Mapping[str, Any]) -> Dict[str, Any]:
    return {k: v for k, v in data.items() if v is not None}


def index_doc_in(doc: Any) -> Dict[str, Any]:
    if isinstance(doc, Mapping):
        doc = IndexDoc(**{k: v for k, v in doc.items()})
    return _drop_none(
        {
            "id": doc.id,
            "name": doc.name,
            "body": doc.body,
            "tags": doc.tags,
            "vector": vector_in(doc.vector),
            "filters": filters_in(doc.filters),
        }
    )


def fusion_in(fusion: Any) -> Optional[Dict[str, Any]]:
    if fusion is None:
        return None
    if isinstance(fusion, str):
        return {"kind": fusion}
    if isinstance(fusion, Fusion):
        return _drop_none({"kind": fusion.kind, "k": fusion.k})
    return dict(fusion)


def boosts_in(boosts: Any) -> Optional[Dict[str, Any]]:
    if boosts is None:
        return None
    if isinstance(boosts, FieldBoosts):
        return _drop_none(asdict(boosts))
    return dict(boosts)


def scan_in(options: Any) -> Optional[Dict[str, Any]]:
    if options is None:
        return None
    if isinstance(options, ScanOptions):
        return _drop_none(asdict(options))
    return dict(options)


def pattern_in(pattern: Any) -> Dict[str, Any]:
    if pattern is None:
        return {}
    if isinstance(pattern, EventPattern):
        data = _drop_none(
            {"agent_id": pattern.agent_id, "kind": pattern.kind, "stream_id": pattern.stream_id}
        )
        if pattern.predicate is not None:
            data["predicate"] = predicate_in(pattern.predicate)
        return data
    data = dict(pattern)
    if data.get("predicate") is not None:
        data["predicate"] = predicate_in(data["predicate"])
    return data


def predicate_in(predicate: Any) -> Dict[str, Any]:
    data = asdict(predicate) if is_dataclass(predicate) else dict(predicate)
    if data.get("kind") == "Always":
        return {"kind": "Always"}
    return data


def event_out(raw: Mapping[str, Any]) -> Event:
    return Event(**raw)


def hit_out(raw: Mapping[str, Any]) -> Hit:
    return Hit(**raw)


def doc_entry_out(raw: Mapping[str, Any]) -> DocEntry:
    return DocEntry(**raw)


def decision_out(raw: Mapping[str, Any]) -> Decision:
    return Decision(**raw)


def tool_stats_out(raw: Optional[Mapping[str, Any]]) -> Optional[ToolStats]:
    return None if raw is None else ToolStats(**raw)


def trace_out(raw: Mapping[str, Any]) -> VectorTrace:
    return VectorTrace(
        hits=[hit_out(h) for h in raw["hits"]],
        steps=[
            TraceStep(layer=s["layer"], node=s["node"], distance=s["distance"], from_=s["from"])
            for s in raw["steps"]
        ],
    )


def progress_out(raw: Mapping[str, Any]) -> ModelProgress:
    return ModelProgress(**raw)


def prepared_out(raw: Mapping[str, Any]) -> PreparedEmbedder:
    return PreparedEmbedder(**raw)
