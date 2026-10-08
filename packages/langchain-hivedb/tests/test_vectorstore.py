from typing import Iterator

import pytest
from hivedb import HiveDB
from langchain_core.embeddings import DeterministicFakeEmbedding
from langchain_core.vectorstores import VectorStore
from langchain_tests.integration_tests import VectorStoreIntegrationTests

from langchain_hivedb import HiveDBVectorStore

DIMENSION = 6


class TestHiveDBVectorStoreStandard(VectorStoreIntegrationTests):
    """Suite de contrato oficial de LangChain."""

    @pytest.fixture()
    def vectorstore(self) -> Iterator[VectorStore]:
        db = HiveDB.open(":memory:", vector={"dimension": DIMENSION, "space_id": "fake-6d"})
        store = HiveDBVectorStore(db, DeterministicFakeEmbedding(size=DIMENSION), hybrid=False)
        yield store
        db.close()

    @property
    def has_sync(self) -> bool:
        return True

    @property
    def has_async(self) -> bool:
        return True
