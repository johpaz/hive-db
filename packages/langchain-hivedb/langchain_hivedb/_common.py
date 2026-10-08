"""Piezas compartidas por los adaptadores."""

from __future__ import annotations

from typing import Any, Dict, List, Mapping, Optional

#: Campo reservado de filtro: separa lo que guarda cada adaptador cuando comparten una base.
SCOPE_FIELD = "_scope"

#: Separadores de identificadores (caracteres de control que no aparecen en texto normal).
UNIT = "\x1f"
RECORD = "\x1e"
GROUP = "\x1d"


def scalar_filters(scope: str, filter: Optional[Mapping[str, Any]] = None) -> List[Dict[str, str]]:
    """Filtros de HiveDB (igualdad de texto) para un ámbito y un `filter` de metadatos escalares."""
    filters = [{"field": SCOPE_FIELD, "value": scope}]
    for field, value in (filter or {}).items():
        filters.append({"field": str(field), "value": scalar_text(value)})
    return filters


def scalar_text(value: Any) -> str:
    """Representación textual estable de un escalar (las comparaciones de HiveDB son de texto)."""
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def is_scalar(value: Any) -> bool:
    return isinstance(value, (str, int, float, bool)) and value is not None
