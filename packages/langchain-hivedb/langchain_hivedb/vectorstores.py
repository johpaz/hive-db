"""`VectorStore` de LangChain sobre HiveDB."""

from __future__ import annotations

import uuid
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence, Tuple, Type, TypeVar

from hivedb import HiveDB
from langchain_core.documents import Document
from langchain_core.embeddings import Embeddings
from langchain_core.vectorstores import VectorStore

from ._common import GROUP, SCOPE_FIELD, is_scalar, scalar_filters, scalar_text

VST = TypeVar("VST", bound="HiveDBVectorStore")


class HiveDBVectorStore(VectorStore):
    """Almacén de documentos con búsqueda híbrida (BM25 + vectores, fusión RRF).

    Dos formas de obtener los embeddings:

    * `embedding=None` (recomendada): abre la base con `HiveDB.open(path, embedder="local")` y
      HiveDB embebe los textos y las consultas dentro del motor (multilingüe, sin API externa).
    * `embedding=<Embeddings de LangChain>`: los vectores los calcula LangChain; abre la base
      con `vector={"dimension": N, "space_id": "..."}`.

    `similarity_search*` usa búsqueda híbrida (texto + significado). Con `hybrid=False` solo
    significado. Las puntuaciones son «mayor = más parecido» (coseno si solo hay vector, RRF si es
    híbrida).
    """

    def __init__(
        self,
        db: HiveDB,
        embedding: Optional[Embeddings] = None,
        *,
        collection_name: str = "lc_documents",
        hybrid: bool = True,
    ) -> None:
        self._db = db
        self._embedding = embedding
        self._collection = db.collection(collection_name)
        self._scope = collection_name
        self._hybrid = hybrid

    # -- utilidades ---------------------------------------------------------------------------

    @property
    def embeddings(self) -> Optional[Embeddings]:
        return self._embedding

    def _index_id(self, id: str) -> str:
        return f"{self._scope}{GROUP}{id}"

    def _filters(self, filter: Optional[Dict[str, Any]]) -> List[Dict[str, str]]:
        return scalar_filters(self._scope, filter)

    def _embed_documents(self, texts: List[str]) -> List[Optional[List[float]]]:
        if self._embedding is None:
            return [None] * len(texts)
        return [list(map(float, v)) for v in self._embedding.embed_documents(texts)]

    # -- escritura ----------------------------------------------------------------------------

    def add_texts(
        self,
        texts: Iterable[str],
        metadatas: Optional[List[dict]] = None,
        *,
        ids: Optional[List[str]] = None,
        **kwargs: Any,
    ) -> List[str]:
        texts = list(texts)
        if metadatas is not None and len(metadatas) != len(texts):
            raise ValueError("metadatas debe tener la misma longitud que texts")
        if ids is not None and len(ids) != len(texts):
            raise ValueError("ids debe tener la misma longitud que texts")
        # `ids` puede traer huecos (`None`) cuando solo algunos documentos tienen id.
        out_ids = [i if i is not None else str(uuid.uuid4()) for i in (ids or [None] * len(texts))]
        vectors = self._embed_documents(texts)
        docs = []
        for i, text in enumerate(texts):
            metadata = (metadatas[i] if metadatas else None) or {}
            self._collection.put(out_ids[i], {"page_content": text, "metadata": metadata})
            filters = [{"field": SCOPE_FIELD, "value": self._scope}]
            filters += [
                {"field": str(k), "value": scalar_text(v)}
                for k, v in metadata.items()
                if is_scalar(v) and k != SCOPE_FIELD
            ]
            docs.append(
                {
                    "id": self._index_id(out_ids[i]),
                    "body": text,
                    "vector": vectors[i],
                    "filters": filters,
                }
            )
        self._db.upsert_batch(docs)
        return out_ids

    def delete(self, ids: Optional[List[str]] = None, **kwargs: Any) -> Optional[bool]:
        if ids is None:
            for entry in self._collection.scan():
                self._remove(entry.id)
            return True
        for id in ids:
            self._remove(id)
        return True

    def _remove(self, id: str) -> None:
        self._collection.delete(id)
        self._db.delete_doc(self._index_id(id))

    # -- lectura ------------------------------------------------------------------------------

    def get_by_ids(self, ids: Sequence[str], /) -> List[Document]:
        documents = []
        for id in ids:
            entry = self._collection.get(id)
            if entry is not None:
                documents.append(self._to_document(entry.id, entry.doc))
        return documents

    @staticmethod
    def _to_document(id: str, doc: Dict[str, Any]) -> Document:
        return Document(id=id, page_content=doc["page_content"], metadata=doc.get("metadata") or {})

    def _search(
        self,
        text: Optional[str],
        vector: Optional[List[float]],
        k: int,
        filter: Optional[Dict[str, Any]],
    ) -> List[Tuple[Document, float]]:
        hits = self._db.query_hybrid(text=text, vector=vector, k=k, filters=self._filters(filter))
        prefix = f"{self._scope}{GROUP}"
        results: List[Tuple[Document, float]] = []
        for hit in hits:
            if not hit.id.startswith(prefix):
                continue
            entry = self._collection.get(hit.id[len(prefix) :])
            if entry is not None:
                results.append((self._to_document(entry.id, entry.doc), hit.score))
        return results

    def _query_parts(self, query: str) -> Tuple[Optional[str], Optional[List[float]]]:
        if self._embedding is None:
            # HiveDB embebe la consulta de texto por su cuenta si la base tiene embedder.
            return query, None
        vector = list(map(float, self._embedding.embed_query(query)))
        return (query if self._hybrid else None), vector

    def similarity_search_with_score(
        self, query: str, k: int = 4, filter: Optional[Dict[str, Any]] = None, **kwargs: Any
    ) -> List[Tuple[Document, float]]:
        text, vector = self._query_parts(query)
        return self._search(text, vector, k, filter)

    def similarity_search(
        self, query: str, k: int = 4, filter: Optional[Dict[str, Any]] = None, **kwargs: Any
    ) -> List[Document]:
        return [d for d, _ in self.similarity_search_with_score(query, k, filter, **kwargs)]

    def similarity_search_by_vector(
        self,
        embedding: List[float],
        k: int = 4,
        filter: Optional[Dict[str, Any]] = None,
        **kwargs: Any,
    ) -> List[Document]:
        return [d for d, _ in self._search(None, list(map(float, embedding)), k, filter)]

    def _select_relevance_score_fn(self) -> Callable[[float], float]:
        # Las puntuaciones de HiveDB ya son «mayor = más parecido».
        return lambda score: score

    # -- construcción -------------------------------------------------------------------------

    @classmethod
    def from_texts(
        cls: Type[VST],
        texts: List[str],
        embedding: Optional[Embeddings] = None,
        metadatas: Optional[List[dict]] = None,
        *,
        ids: Optional[List[str]] = None,
        db: Optional[HiveDB] = None,
        collection_name: str = "lc_documents",
        hybrid: bool = True,
        **kwargs: Any,
    ) -> VST:
        """Crea el almacén y añade `texts`. `db` es obligatorio (una base de HiveDB abierta)."""
        if db is None:
            raise ValueError("from_texts necesita `db=` (una base abierta con HiveDB.open)")
        store = cls(db, embedding, collection_name=collection_name, hybrid=hybrid)
        store.add_texts(texts, metadatas, ids=ids)
        return store
