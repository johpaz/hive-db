"""Adaptadores de LangChain y LangGraph para HiveDB."""

from .chat_history import HiveDBChatMessageHistory
from .vectorstores import HiveDBVectorStore

__all__ = ["HiveDBChatMessageHistory", "HiveDBStore", "HiveDBVectorStore"]


def __getattr__(name: str):
    # `HiveDBStore` necesita langgraph-checkpoint (extra `langgraph`): se importa al pedirlo.
    if name == "HiveDBStore":
        from .store import HiveDBStore

        return HiveDBStore
    raise AttributeError(name)
