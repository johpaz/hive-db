"""API síncrona de HiveDB. Cada llamada suelta el GIL mientras trabaja el motor."""

from __future__ import annotations

import threading
from typing import Any, Callable, Dict, Iterable, Iterator, List, Mapping, Optional, Sequence, Union

from . import _convert as c
from . import _native
from .types import (
    BatchOp,
    Decision,
    DocEntry,
    Event,
    EventKind,
    EventPattern,
    FieldBoosts,
    FiltersInput,
    Fusion,
    Hit,
    IndexDoc,
    ModelProgress,
    PreparedEmbedder,
    ScanOptions,
    ToolStats,
    VectorOptions,
    VectorTrace,
)

PatternInput = Union[EventPattern, Mapping[str, Any], None]


class Collection:
    """Colección de documentos JSON con nombre (almacenamiento mutable, separado del registro
    inmutable de eventos). Cada escritura se confirma de forma atómica con sus índices."""

    def __init__(self, native: _native.Database, name: str) -> None:
        self._native = native
        self.name = name

    def put(self, id: str, doc: Any, *, expected_version: Optional[int] = None) -> int:
        """Inserta o reemplaza un documento y devuelve su nueva versión (1 la primera vez).
        Con `expected_version` la versión actual debe coincidir (0 = no debe existir)."""
        return self._native.col_put(self.name, id, doc, expected_version)

    def get(self, id: str) -> Optional[DocEntry]:
        raw = self._native.col_get(self.name, id)
        return None if raw is None else c.doc_entry_out(raw)

    def delete(self, id: str) -> bool:
        """Borra un documento. Devuelve `True` si existía."""
        return self._native.col_delete(self.name, id)

    def scan(self, options: Union[ScanOptions, Mapping[str, Any], None] = None) -> List[DocEntry]:
        """Recorre la colección en orden de id."""
        return [c.doc_entry_out(e) for e in self._native.col_scan(self.name, c.scan_in(options))]

    def count(self) -> int:
        return self._native.col_count(self.name)

    def create_index(self, field: str, *, unique: bool = False) -> None:
        """Índice de igualdad sobre un campo de primer nivel. Rellena los documentos existentes;
        es idempotente para una definición idéntica."""
        self._native.col_create_index(self.name, field, unique)

    def find_by(
        self,
        field: str,
        value: Union[str, int, float, bool],
        options: Union[ScanOptions, Mapping[str, Any], None] = None,
    ) -> List[DocEntry]:
        """Documentos cuyo campo indexado es igual a `value`. Exige `create_index` antes."""
        entries = self._native.col_find_by(self.name, field, value, c.scan_in(options))
        return [c.doc_entry_out(e) for e in entries]


class EventStream:
    """Eventos que coinciden con un patrón. Iterable (bloquea hasta el siguiente), usable en
    `with`, y se cancela con `close()` desde cualquier hilo."""

    def __init__(self, native: _native.Subscription) -> None:
        self._native = native

    def next(self, timeout: Optional[float] = None) -> Optional[Event]:
        """Siguiente evento; `None` si se cerró. Con `timeout` (s) lanza `TimeoutError`."""
        raw = self._native.next(timeout)
        return None if raw is None else c.event_out(raw)

    def close(self) -> None:
        self._native.close()

    def __iter__(self) -> Iterator[Event]:
        return self

    def __next__(self) -> Event:
        event = self.next()
        if event is None:
            raise StopIteration
        return event

    def __enter__(self) -> "EventStream":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


class HiveDB:
    """Base de memoria de agentes. Ábrela con `HiveDB.open(path)`."""

    def __init__(self, native: _native.Database) -> None:
        self._native = native

    # -- apertura -----------------------------------------------------------------------------

    @classmethod
    def open(
        cls,
        path: str,
        *,
        vector: Union[VectorOptions, Mapping[str, Any], None] = None,
        embedder: Optional[str] = None,
    ) -> "HiveDB":
        """Abre (o crea) una base. `":memory:"` abre una base efímera.

        `vector` activa el índice vectorial con embeddings propios. `embedder="local"` genera los
        embeddings dentro de HiveDB (multilingual-e5-small, 384 dimensiones, español e inglés): la
        primera apertura descarga el modelo (~470 MB; usa `HiveDB.prepare_embedder` para verlo
        avanzar) y todas las bases del proceso comparten una sola copia. No hace falta `vector`.
        """
        options: Dict[str, Any] = {}
        if vector is not None:
            if isinstance(vector, VectorOptions):
                options["vector"] = {"dimension": vector.dimension, "space_id": vector.space_id}
            else:
                v = dict(vector)
                options["vector"] = {
                    "dimension": v["dimension"],
                    "space_id": v.get("space_id", v.get("spaceId")),
                }
        if embedder is not None:
            options["embedder"] = embedder
        return cls(_native.Database.open(path, options or None))

    @staticmethod
    def prepare_embedder(
        on_progress: Optional[Callable[[ModelProgress], None]] = None,
    ) -> PreparedEmbedder:
        """Descarga (si falta) y verifica el modelo del embedder local, sin abrir ninguna base.

        La descarga se reanuda si se corta. `HIVEDB_OFFLINE=1` impide el acceso a la red (falla con
        `EMBEDDER_UNAVAILABLE` si el modelo no está), `HIVEDB_MODEL_DIR` cambia la caché y
        `HIVEDB_MODEL_BASE_URL` apunta a un espejo.
        """
        callback = (lambda raw: on_progress(c.progress_out(raw))) if on_progress else None
        return c.prepared_out(_native.prepare_embedder(callback))

    def close(self) -> None:
        """Cierra la base: espera a las operaciones en curso. Idempotente."""
        self._native.close()

    def __enter__(self) -> "HiveDB":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- registro de eventos ------------------------------------------------------------------

    def append(
        self,
        agent_id: str,
        stream_id: str,
        kind: EventKind,
        payload: Any = None,
        *,
        causation: Optional[int] = None,
        correlation: Optional[str] = None,
    ) -> int:
        """Añade un evento al registro y devuelve su `seq`. Algunos tipos exigen campos en
        `payload`: `ToolCall` → `tool`; `MemoryInvalidate` → `target_seq`; `ConsentGranted` →
        `from`, `to`, `action`, `resource`; `ConsentRevoked` → `grant_seq`; `IntentLogged` →
        `actor`, `intent`."""
        return self._native.append(
            {
                "agent_id": agent_id,
                "stream_id": stream_id,
                "kind": kind,
                "payload": payload,
                "causation": causation,
                "correlation": correlation,
            }
        )

    def read(self, seq: int) -> Event:
        return c.event_out(self._native.read(seq))

    def log_len(self) -> int:
        return self._native.log_len()

    def last_seq(self) -> int:
        return self._native.last_seq()

    # -- proyecciones -------------------------------------------------------------------------

    def tool_stats(self, tool: str, agents: Optional[Sequence[str]] = None) -> Optional[ToolStats]:
        """Estadísticas agregadas de una herramienta. En una base compartida por varios
        inquilinos pasa siempre `agents`; sin él se suman las llamadas de todos."""
        return c.tool_stats_out(
            self._native.tool_stats(tool, None if agents is None else list(agents))
        )

    def project_task_state(self, agent_id: str, stream_id: str) -> Optional[str]:
        return self._native.project_task_state(agent_id, stream_id)

    def can(self, agent: str, action: str, resource: str) -> Decision:
        return c.decision_out(self._native.can(agent, action, resource))

    # -- memoria de trabajo -------------------------------------------------------------------

    def working_set(
        self, agent_id: str, key: str, value: Any, ttl_ms: Optional[int] = None
    ) -> None:
        self._native.working_set(agent_id, key, value, ttl_ms)

    def working_get(self, agent_id: str, key: str) -> Any:
        """Valor guardado, o `None` si no existe o expiró."""
        return self._native.working_get(agent_id, key)

    def working_keys(self, agent_id: str) -> List[str]:
        return self._native.working_keys(agent_id)

    # -- contexto y harness -------------------------------------------------------------------

    def causal_thread(self, stream_id: str, agents: Optional[Sequence[str]] = None) -> Any:
        """Hilo causal de un stream. Acótalo con `agents` en bases compartidas."""
        return self._native.causal_thread(stream_id, None if agents is None else list(agents))

    def build_agent_context(self, request: Mapping[str, Any]) -> Any:
        """Ventana de contexto para una tarea. `request` usa las claves del motor
        (`task_id`, `current_phase`, `current_objective`, `max_tokens`, `strategy`, `agents`)."""
        return self._native.build_agent_context(dict(request))

    @staticmethod
    def evaluate_harness(input: Mapping[str, Any]) -> Any:
        return _native.evaluate_harness(dict(input))

    # -- índice semántico ---------------------------------------------------------------------

    def upsert_doc(
        self,
        id: Union[str, IndexDoc, Mapping[str, Any]],
        *,
        name: Optional[str] = None,
        body: Optional[str] = None,
        tags: Optional[str] = None,
        vector: Optional[Sequence[float]] = None,
        filters: Optional[FiltersInput] = None,
    ) -> None:
        """Inserta o reemplaza un documento del índice. Con `embedder="local"`, un documento sin
        `vector` se embebe a partir de su texto."""
        if isinstance(id, IndexDoc):
            doc = id
        elif isinstance(id, Mapping):
            doc = IndexDoc(**id)
        else:
            doc = IndexDoc(id, name, body, tags, vector, filters)
        self._native.upsert_doc(c.index_doc_in(doc))

    def upsert_batch(self, docs: Iterable[Union[IndexDoc, Mapping[str, Any]]]) -> None:
        """Inserta o reemplaza varios documentos con una sola confirmación del índice de texto
        (mucho más rápido que repetir `upsert_doc`)."""
        self._native.upsert_batch([c.index_doc_in(d) for d in docs])

    def delete_doc(self, id: str) -> None:
        self._native.delete_doc(id)

    def delete_by_filter(self, field: str, value: str) -> None:
        self._native.delete_by_filter({"field": field, "value": value})

    def clear_index(self) -> None:
        self._native.clear_index()

    def compact_index(self) -> None:
        """Reconstruye los índices semánticos desde sus documentos autoritativos."""
        self._native.compact_index()

    def query_hybrid(
        self,
        *,
        text: Optional[str] = None,
        vector: Optional[Sequence[float]] = None,
        k: int = 10,
        filters: Optional[FiltersInput] = None,
        fusion: Union[Fusion, Mapping[str, Any], str, None] = None,
        boosts: Union[FieldBoosts, Mapping[str, Any], None] = None,
        ef_search: Optional[int] = None,
    ) -> List[Hit]:
        """Búsqueda por texto (BM25), por vector o híbrida (RRF) con filtros escalares.
        `ef_search` sube el recall a costa de latencia (por defecto 200)."""
        query: Dict[str, Any] = {"k": k}
        for key, value in (
            ("text", text),
            ("vector", c.vector_in(vector)),
            ("filters", c.filters_in(filters)),
            ("fusion", c.fusion_in(fusion)),
            ("boosts", c.boosts_in(boosts)),
            ("ef_search", ef_search),
        ):
            if value is not None:
                query[key] = value
        return [c.hit_out(h) for h in self._native.query_hybrid(query)]

    def trace_vector(
        self, vector: Sequence[float], k: int = 10, ef_search: Optional[int] = None
    ) -> VectorTrace:
        """Búsqueda vectorial sin filtros devolviendo la ruta del HNSW."""
        return c.trace_out(self._native.trace_vector(c.vector_in(vector) or [], k, ef_search))

    # -- colecciones --------------------------------------------------------------------------

    def collection(self, name: str) -> Collection:
        return Collection(self._native, name)

    def batch(self, ops: Iterable[BatchOp]) -> None:
        """Aplica altas y bajas de forma atómica: o se confirman todas o ninguna.
        Cada operación es `{"op": "put", "collection", "id", "doc", "expected_version"?}` o
        `{"op": "delete", "collection", "id"}`."""
        self._native.col_batch([dict(op) for op in ops])

    # -- suscripciones ------------------------------------------------------------------------

    def events(self, pattern: PatternInput = None) -> EventStream:
        """Flujo de eventos que coinciden con `pattern`: `for event in db.events(...)`."""
        return EventStream(self._native.subscribe(c.pattern_in(pattern)))

    def subscribe(self, pattern: PatternInput, on_event: Callable[[Event], None]) -> EventStream:
        """Llama a `on_event` en un hilo en segundo plano por cada evento. Devuelve el flujo:
        `close()` detiene la suscripción."""
        stream = self.events(pattern)

        def pump() -> None:
            for event in stream:
                try:
                    on_event(event)
                except Exception:  # un fallo del callback no debe matar la suscripción
                    pass

        threading.Thread(target=pump, name="hivedb-subscriber", daemon=True).start()
        return stream
