"""HiveDB: base de memoria embebida para agentes de IA.

from hivedb import HiveDB

with HiveDB.open("./memoria") as db:
    db.upsert_doc("nota-1", body="El cliente prefiere factura electrónica")
    print(db.query_hybrid(text="factura", k=5))
"""

from ._native import ERROR_CODES, HiveDBError
from .aio import AsyncCollection, AsyncEventStream, AsyncHiveDB
from .database import Collection, EventStream, HiveDB
from .errors import error_code
from .types import (
    BatchOp,
    Decision,
    DocEntry,
    Event,
    EventKind,
    EventPattern,
    FieldBoosts,
    Fusion,
    Hit,
    IndexDoc,
    ModelProgress,
    Predicate,
    PreparedEmbedder,
    ScalarFilter,
    ScanOptions,
    ToolStats,
    TraceStep,
    VectorOptions,
    VectorTrace,
)

__all__ = [
    "AsyncCollection",
    "AsyncEventStream",
    "AsyncHiveDB",
    "BatchOp",
    "Collection",
    "Decision",
    "DocEntry",
    "ERROR_CODES",
    "Event",
    "EventKind",
    "EventPattern",
    "EventStream",
    "FieldBoosts",
    "Fusion",
    "Hit",
    "HiveDB",
    "HiveDBError",
    "IndexDoc",
    "ModelProgress",
    "Predicate",
    "PreparedEmbedder",
    "ScalarFilter",
    "ScanOptions",
    "ToolStats",
    "TraceStep",
    "VectorOptions",
    "VectorTrace",
    "error_code",
]
