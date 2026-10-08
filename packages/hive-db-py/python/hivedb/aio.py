"""Versión asyncio de la API: cada llamada corre en un hilo (`asyncio.to_thread`) y el motor
suelta el GIL, así que varias consultas concurrentes se reparten por los núcleos."""

from __future__ import annotations

import asyncio
from typing import Any, AsyncIterator, Callable, Iterable, List, Mapping, Optional, Sequence, Union

from .database import Collection, EventStream, HiveDB, PatternInput
from .types import (
    BatchOp,
    Decision,
    DocEntry,
    Event,
    EventKind,
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


class AsyncCollection:
    def __init__(self, inner: Collection) -> None:
        self._inner = inner
        self.name = inner.name

    async def put(self, id: str, doc: Any, *, expected_version: Optional[int] = None) -> int:
        return await asyncio.to_thread(self._inner.put, id, doc, expected_version=expected_version)

    async def get(self, id: str) -> Optional[DocEntry]:
        return await asyncio.to_thread(self._inner.get, id)

    async def delete(self, id: str) -> bool:
        return await asyncio.to_thread(self._inner.delete, id)

    async def scan(
        self, options: Union[ScanOptions, Mapping[str, Any], None] = None
    ) -> List[DocEntry]:
        return await asyncio.to_thread(self._inner.scan, options)

    async def count(self) -> int:
        return await asyncio.to_thread(self._inner.count)

    async def create_index(self, field: str, *, unique: bool = False) -> None:
        await asyncio.to_thread(self._inner.create_index, field, unique=unique)

    async def find_by(
        self,
        field: str,
        value: Union[str, int, float, bool],
        options: Union[ScanOptions, Mapping[str, Any], None] = None,
    ) -> List[DocEntry]:
        return await asyncio.to_thread(self._inner.find_by, field, value, options)


class AsyncEventStream:
    """`async for event in db.events(...)`; `close()` termina el recorrido."""

    _POLL = 0.2

    def __init__(self, inner: EventStream) -> None:
        self._inner = inner
        self._closed = False

    def close(self) -> None:
        self._closed = True
        self._inner.close()

    def __aiter__(self) -> AsyncIterator[Event]:
        return self

    async def __anext__(self) -> Event:
        while not self._closed:
            try:
                event = await asyncio.to_thread(self._inner.next, self._POLL)
            except TimeoutError:
                continue
            if event is None:
                break
            return event
        raise StopAsyncIteration

    async def __aenter__(self) -> "AsyncEventStream":
        return self

    async def __aexit__(self, *exc: object) -> None:
        self.close()


class AsyncHiveDB:
    """Misma API que `HiveDB`, con `await`."""

    def __init__(self, inner: HiveDB) -> None:
        self._inner = inner

    @classmethod
    async def open(
        cls,
        path: str,
        *,
        vector: Union[VectorOptions, Mapping[str, Any], None] = None,
        embedder: Optional[str] = None,
    ) -> "AsyncHiveDB":
        inner = await asyncio.to_thread(HiveDB.open, path, vector=vector, embedder=embedder)
        return cls(inner)

    @staticmethod
    async def prepare_embedder(
        on_progress: Optional[Callable[[ModelProgress], None]] = None,
    ) -> PreparedEmbedder:
        """`on_progress` se llama desde un hilo de trabajo, no desde el bucle de eventos."""
        return await asyncio.to_thread(HiveDB.prepare_embedder, on_progress)

    async def close(self) -> None:
        await asyncio.to_thread(self._inner.close)

    async def __aenter__(self) -> "AsyncHiveDB":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()

    async def append(
        self,
        agent_id: str,
        stream_id: str,
        kind: EventKind,
        payload: Any = None,
        *,
        causation: Optional[int] = None,
        correlation: Optional[str] = None,
    ) -> int:
        return await asyncio.to_thread(
            self._inner.append,
            agent_id,
            stream_id,
            kind,
            payload,
            causation=causation,
            correlation=correlation,
        )

    async def read(self, seq: int) -> Event:
        return await asyncio.to_thread(self._inner.read, seq)

    async def log_len(self) -> int:
        return await asyncio.to_thread(self._inner.log_len)

    async def last_seq(self) -> int:
        return await asyncio.to_thread(self._inner.last_seq)

    async def tool_stats(
        self, tool: str, agents: Optional[Sequence[str]] = None
    ) -> Optional[ToolStats]:
        return await asyncio.to_thread(self._inner.tool_stats, tool, agents)

    async def project_task_state(self, agent_id: str, stream_id: str) -> Optional[str]:
        return await asyncio.to_thread(self._inner.project_task_state, agent_id, stream_id)

    async def can(self, agent: str, action: str, resource: str) -> Decision:
        return await asyncio.to_thread(self._inner.can, agent, action, resource)

    async def working_set(
        self, agent_id: str, key: str, value: Any, ttl_ms: Optional[int] = None
    ) -> None:
        await asyncio.to_thread(self._inner.working_set, agent_id, key, value, ttl_ms)

    async def working_get(self, agent_id: str, key: str) -> Any:
        return await asyncio.to_thread(self._inner.working_get, agent_id, key)

    async def working_keys(self, agent_id: str) -> List[str]:
        return await asyncio.to_thread(self._inner.working_keys, agent_id)

    async def causal_thread(self, stream_id: str, agents: Optional[Sequence[str]] = None) -> Any:
        return await asyncio.to_thread(self._inner.causal_thread, stream_id, agents)

    async def build_agent_context(self, request: Mapping[str, Any]) -> Any:
        return await asyncio.to_thread(self._inner.build_agent_context, request)

    async def upsert_doc(
        self,
        id: Union[str, IndexDoc, Mapping[str, Any]],
        *,
        name: Optional[str] = None,
        body: Optional[str] = None,
        tags: Optional[str] = None,
        vector: Optional[Sequence[float]] = None,
        filters: Optional[FiltersInput] = None,
    ) -> None:
        await asyncio.to_thread(
            self._inner.upsert_doc,
            id,
            name=name,
            body=body,
            tags=tags,
            vector=vector,
            filters=filters,
        )

    async def upsert_batch(self, docs: Iterable[Union[IndexDoc, Mapping[str, Any]]]) -> None:
        await asyncio.to_thread(self._inner.upsert_batch, list(docs))

    async def delete_doc(self, id: str) -> None:
        await asyncio.to_thread(self._inner.delete_doc, id)

    async def delete_by_filter(self, field: str, value: str) -> None:
        await asyncio.to_thread(self._inner.delete_by_filter, field, value)

    async def clear_index(self) -> None:
        await asyncio.to_thread(self._inner.clear_index)

    async def compact_index(self) -> None:
        await asyncio.to_thread(self._inner.compact_index)

    async def query_hybrid(
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
        return await asyncio.to_thread(
            self._inner.query_hybrid,
            text=text,
            vector=vector,
            k=k,
            filters=filters,
            fusion=fusion,
            boosts=boosts,
            ef_search=ef_search,
        )

    async def trace_vector(
        self, vector: Sequence[float], k: int = 10, ef_search: Optional[int] = None
    ) -> VectorTrace:
        return await asyncio.to_thread(self._inner.trace_vector, vector, k, ef_search)

    def collection(self, name: str) -> AsyncCollection:
        return AsyncCollection(self._inner.collection(name))

    async def batch(self, ops: Iterable[BatchOp]) -> None:
        await asyncio.to_thread(self._inner.batch, list(ops))

    def events(self, pattern: PatternInput = None) -> AsyncEventStream:
        return AsyncEventStream(self._inner.events(pattern))
