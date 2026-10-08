"""`BaseStore` de LangGraph sobre HiveDB: memoria a largo plazo de los agentes."""

from __future__ import annotations

import asyncio
import operator
from datetime import datetime, timezone
from typing import Any, Dict, Iterable, List, Optional, Tuple

from hivedb import HiveDB
from langgraph.store.base import (
    BaseStore,
    GetOp,
    Item,
    ListNamespacesOp,
    Op,
    PutOp,
    Result,
    SearchItem,
    SearchOp,
)

from ._common import GROUP, RECORD, SCOPE_FIELD, UNIT

#: Niveles de namespace que se indexan como filtros para acotar la búsqueda semántica por prefijo.
MAX_NAMESPACE_DEPTH = 16


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _check_part(part: str) -> str:
    if not isinstance(part, str) or not part or UNIT in part or RECORD in part:
        raise ValueError(f"parte de namespace/clave no válida: {part!r}")
    return part


def _item_id(namespace: Tuple[str, ...], key: str) -> str:
    return UNIT.join(_check_part(p) for p in namespace) + RECORD + _check_part(key)


def _ns_prefix(prefix: Tuple[str, ...]) -> str:
    return UNIT.join(_check_part(p) for p in prefix)


def _in_prefix(namespace: Tuple[str, ...], prefix: Tuple[str, ...]) -> bool:
    return namespace[: len(prefix)] == prefix


def _texts(value: Any) -> List[str]:
    """Todas las cadenas hoja de un valor JSON."""
    if isinstance(value, str):
        return [value]
    if isinstance(value, dict):
        return [t for v in value.values() for t in _texts(v)]
    if isinstance(value, list):
        return [t for v in value for t in _texts(v)]
    return []


def _field(value: Any, path: str) -> List[Any]:
    """Valores en una ruta con puntos; `[*]` / `*` recorre listas."""
    nodes = [value]
    for part in path.split("."):
        part = part.replace("[*]", "")
        next_nodes: List[Any] = []
        for node in nodes:
            if part in ("", "*"):
                next_nodes.extend(node if isinstance(node, list) else [node])
            elif isinstance(node, dict) and part in node:
                child = node[part]
                next_nodes.extend(
                    child if isinstance(child, list) and path.endswith("[*]") else [child]
                )
        nodes = next_nodes
    return nodes


_OPERATORS = {
    "$eq": operator.eq,
    "$ne": operator.ne,
    "$gt": operator.gt,
    "$gte": operator.ge,
    "$lt": operator.lt,
    "$lte": operator.le,
}


def _matches(value: Dict[str, Any], filter: Dict[str, Any]) -> bool:
    for key, expected in filter.items():
        actual = value.get(key)
        if isinstance(expected, dict) and expected and all(k.startswith("$") for k in expected):
            for name, operand in expected.items():
                compare = _OPERATORS.get(name)
                if compare is None:
                    return False
                try:
                    if not compare(actual, operand):
                        return False
                except TypeError:
                    return False
        elif actual != expected:
            return False
    return True


class HiveDBStore(BaseStore):
    """Memoria a largo plazo de LangGraph (`store=`) persistida en HiveDB.

    * Los elementos viven en una colección de HiveDB (`put`/`get`/`delete`/`list_namespaces`).
    * `search(prefix, query=...)` es **semántica/híbrida**: HiveDB indexa el texto de cada
      elemento (BM25 y, si la base se abrió con `embedder="local"`, también por significado).
      Sin `query` recorre el namespace en orden de clave.
    * `put(..., index=False)` excluye el elemento de la búsqueda; `index=[...]` indexa solo esos
      campos (rutas con puntos); por defecto se indexan todas las cadenas del valor.

    No admite TTL. `list_namespaces` y las búsquedas con `filter` recorren los elementos del
    namespace (coste lineal en su tamaño): para almacenes muy grandes usa un namespace por tema.
    """

    supports_ttl = False

    def __init__(
        self,
        db: HiveDB,
        *,
        collection_name: str = "langgraph_store",
        index_fields: Optional[List[str]] = None,
    ) -> None:
        self._db = db
        self._collection = db.collection(collection_name)
        self._scope = collection_name
        self._index_fields = index_fields

    # -- índice semántico ---------------------------------------------------------------------

    def _index_key(self, id: str) -> str:
        return f"{self._scope}{GROUP}{id}"

    def _index_text(self, value: Dict[str, Any], fields: Optional[List[str]]) -> str:
        fields = fields if fields is not None else self._index_fields
        if fields is None:
            return "\n".join(_texts(value))
        parts: List[str] = []
        for path in fields:
            parts.extend(t for v in _field(value, path) for t in _texts(v))
        return "\n".join(parts)

    def _index(
        self, namespace: Tuple[str, ...], id: str, value: Dict[str, Any], fields: Any
    ) -> None:
        text = self._index_text(value, fields)
        if not text.strip():
            self._db.delete_doc(self._index_key(id))
            return
        filters = [{"field": SCOPE_FIELD, "value": self._scope}]
        for depth, part in enumerate(namespace[:MAX_NAMESPACE_DEPTH]):
            filters.append({"field": f"ns{depth}", "value": part})
        self._db.upsert_doc({"id": self._index_key(id), "body": text, "filters": filters})

    # -- operaciones --------------------------------------------------------------------------

    def _get(self, op: GetOp) -> Optional[Item]:
        entry = self._collection.get(_item_id(op.namespace, op.key))
        return None if entry is None else self._to_item(entry.doc)

    @staticmethod
    def _to_item(doc: Dict[str, Any], score: Optional[float] = None) -> Item:
        kwargs = dict(
            value=doc["value"],
            key=doc["key"],
            namespace=tuple(doc["namespace"]),
            created_at=datetime.fromisoformat(doc["created_at"]),
            updated_at=datetime.fromisoformat(doc["updated_at"]),
        )
        return SearchItem(score=score, **kwargs) if score is not None else Item(**kwargs)

    def _put(self, op: PutOp) -> None:
        id = _item_id(op.namespace, op.key)
        if op.value is None:
            self._collection.delete(id)
            self._db.delete_doc(self._index_key(id))
            return
        existing = self._collection.get(id)
        now = _now()
        doc = {
            "namespace": list(op.namespace),
            "key": op.key,
            "value": op.value,
            "created_at": existing.doc["created_at"] if existing else now,
            "updated_at": now,
        }
        self._collection.put(id, doc)
        if op.index is False:
            self._db.delete_doc(self._index_key(id))
        else:
            self._index(op.namespace, id, op.value, op.index)

    def _candidates(self, prefix: Tuple[str, ...]) -> Iterable[Dict[str, Any]]:
        scan = {"prefix": _ns_prefix(prefix)} if prefix else None
        for entry in self._collection.scan(scan):
            if _in_prefix(tuple(entry.doc["namespace"]), prefix):
                yield entry.doc

    def _search(self, op: SearchOp) -> List[SearchItem]:
        wanted = op.limit + op.offset
        if op.query:
            filters = [{"field": SCOPE_FIELD, "value": self._scope}]
            indexable = op.namespace_prefix[:MAX_NAMESPACE_DEPTH]
            filters += [{"field": f"ns{i}", "value": p} for i, p in enumerate(indexable)]
            # Con `filter` sobre el valor hay que descartar a posteriori: se sobremuestrea.
            k = wanted * (5 if op.filter else 1)
            hits = self._db.query_hybrid(text=op.query, k=max(k, 1), filters=filters)
            prefix = f"{self._scope}{GROUP}"
            results: List[SearchItem] = []
            for hit in hits:
                if not hit.id.startswith(prefix):
                    continue
                entry = self._collection.get(hit.id[len(prefix) :])
                if entry is None:
                    continue
                doc = entry.doc
                if not _in_prefix(tuple(doc["namespace"]), op.namespace_prefix):
                    continue
                if op.filter and not _matches(doc["value"], op.filter):
                    continue
                results.append(self._to_item(doc, score=hit.score))  # type: ignore[arg-type]
            return results[op.offset : op.offset + op.limit]
        docs = [
            d
            for d in self._candidates(op.namespace_prefix)
            if not op.filter or _matches(d["value"], op.filter)
        ]
        return [self._to_item(d, score=None) for d in docs[op.offset : op.offset + op.limit]]  # type: ignore[misc]

    def _list_namespaces(self, op: ListNamespacesOp) -> List[Tuple[str, ...]]:
        seen = set()
        for doc in self._collection.scan():
            ns = tuple(doc.doc["namespace"])
            if all(self._match(ns, c.match_type, c.path) for c in (op.match_conditions or ())):
                seen.add(ns[: op.max_depth] if op.max_depth is not None else ns)
        ordered = sorted(seen)
        return ordered[op.offset : op.offset + op.limit]

    @staticmethod
    def _match(ns: Tuple[str, ...], match_type: str, path: Tuple[str, ...]) -> bool:
        if len(ns) < len(path):
            return False
        window = ns[: len(path)] if match_type == "prefix" else ns[len(ns) - len(path) :]
        return all(p == "*" or p == n for p, n in zip(path, window))

    # -- BaseStore ----------------------------------------------------------------------------

    def batch(self, ops: Iterable[Op]) -> List[Result]:
        results: List[Result] = []
        for op in ops:
            if isinstance(op, GetOp):
                results.append(self._get(op))
            elif isinstance(op, SearchOp):
                results.append(self._search(op))
            elif isinstance(op, PutOp):
                self._put(op)
                results.append(None)
            elif isinstance(op, ListNamespacesOp):
                results.append(self._list_namespaces(op))
            else:  # pragma: no cover
                raise ValueError(f"operación desconocida: {type(op).__name__}")
        return results

    async def abatch(self, ops: Iterable[Op]) -> List[Result]:
        return await asyncio.to_thread(self.batch, list(ops))


__all__ = ["HiveDBStore"]
