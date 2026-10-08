"""Tipos públicos de HiveDB."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Literal, Mapping, Optional, Sequence, Tuple, Union

EventKind = Literal[
    "Fact",
    "StateTransition",
    "MemoryInvalidate",
    "ToolCall",
    "ConsentGranted",
    "ConsentRevoked",
    "IntentLogged",
    "LearningProposal",
]


@dataclass(frozen=True)
class VectorOptions:
    """Configuración del índice vectorial (fija durante la vida de la base)."""

    dimension: int
    #: Identidad estable del modelo y configuración que produjo los vectores.
    space_id: str


@dataclass(frozen=True)
class ScalarFilter:
    field: str
    value: str


#: Filtros aceptados: `ScalarFilter`, `(campo, valor)`, `{"field", "value"}` o `{campo: valor}`.
FiltersInput = Union[
    Mapping[str, str],
    Sequence[Union[ScalarFilter, Tuple[str, str], Mapping[str, str]]],
]


@dataclass(frozen=True)
class Event:
    seq: int
    agent_id: str
    stream_id: str
    kind_tag: str
    timestamp: int
    payload: Any
    causation: Optional[int] = None
    correlation: Optional[str] = None


@dataclass(frozen=True)
class Decision:
    allowed: bool
    intent_log_seq: Optional[int] = None


@dataclass(frozen=True)
class ToolStats:
    invocations: int
    errors: int
    total_latency_ms: int
    total_cost: float
    last_seq: int
    last_outcome: Optional[str] = None


@dataclass
class IndexDoc:
    """Documento del índice semántico. Todos los campos de texto son opcionales."""

    id: str
    #: Título corto y de mucha señal (el que más pesa por defecto).
    name: Optional[str] = None
    #: Contenido principal.
    body: Optional[str] = None
    #: Categorías, disparadores, palabras clave.
    tags: Optional[str] = None
    #: Embedding opcional; debe coincidir con la dimensión del índice.
    vector: Optional[Sequence[float]] = None
    filters: Optional[FiltersInput] = None


@dataclass(frozen=True)
class Fusion:
    """Fusión de resultados cuando hay texto y vector. Solo `rrf`."""

    kind: Literal["rrf"] = "rrf"
    k: Optional[int] = None


@dataclass(frozen=True)
class FieldBoosts:
    """Pesos BM25 por campo (por defecto: name 4.0, body 2.0, tags 3.0)."""

    name: Optional[float] = None
    body: Optional[float] = None
    tags: Optional[float] = None


@dataclass(frozen=True)
class Hit:
    """Resultado de una búsqueda. Texto: BM25; vector: coseno; híbrida: RRF con los
    componentes crudos en `text_score` / `vector_score`."""

    id: str
    score: float
    text_score: Optional[float] = None
    vector_score: Optional[float] = None


@dataclass(frozen=True)
class TraceStep:
    layer: int
    node: str
    #: Distancia coseno (1 − similitud).
    distance: float
    #: `None` para el punto de entrada.
    from_: Optional[str] = None


@dataclass(frozen=True)
class VectorTrace:
    hits: List[Hit]
    steps: List[TraceStep]


@dataclass(frozen=True)
class DocEntry:
    id: str
    #: Versión monótona por documento; 1 en el primer `put`.
    version: int
    doc: Any


@dataclass(frozen=True)
class ScanOptions:
    prefix: Optional[str] = None
    start: Optional[str] = None
    limit: Optional[int] = None
    offset: Optional[int] = None
    reverse: Optional[bool] = None


@dataclass(frozen=True)
class Predicate:
    kind: Literal["Eq", "Contains", "Always"]
    path: Optional[str] = None
    value: Any = None


@dataclass(frozen=True)
class EventPattern:
    agent_id: Optional[str] = None
    kind: Optional[EventKind] = None
    stream_id: Optional[str] = None
    predicate: Optional[Predicate] = None


@dataclass(frozen=True)
class ModelProgress:
    """Avance de la descarga del modelo del embedder local."""

    file: str
    #: Posición de este archivo entre los que faltan (desde 1).
    file_index: int
    file_count: int
    #: Bytes ya descargados de este archivo (incluye lo reanudado).
    downloaded: int
    total: int


@dataclass(frozen=True)
class PreparedEmbedder:
    dir: str
    space_id: str
    #: `True` si ya estaba todo en la caché y no se descargó nada.
    cached: bool


BatchOp = Dict[str, Any]
