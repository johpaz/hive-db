"""Errores de HiveDB."""

from __future__ import annotations

from typing import Optional

from ._native import ERROR_CODES, HiveDBError

__all__ = ["HiveDBError", "ERROR_CODES", "error_code"]


def error_code(error: BaseException) -> Optional[str]:
    """Código estable de un error de HiveDB (`INVALID_VECTOR`, `VECTOR_SPACE_MISMATCH`,
    `INDEX_DEGRADED`, `EMBEDDER_UNAVAILABLE`) o `None`."""
    return getattr(error, "code", None) if isinstance(error, HiveDBError) else None
