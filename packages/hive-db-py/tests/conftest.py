import pytest
from hivedb import HiveDB


@pytest.fixture
def db():
    database = HiveDB.open(":memory:")
    yield database
    database.close()


@pytest.fixture
def vdb():
    """Base con índice vectorial de 4 dimensiones y embeddings propios."""
    database = HiveDB.open(":memory:", vector={"dimension": 4, "space_id": "test-4d"})
    yield database
    database.close()
