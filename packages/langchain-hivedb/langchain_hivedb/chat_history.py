"""Historial de mensajes de chat de LangChain sobre HiveDB."""

from __future__ import annotations

import time
import uuid
from typing import List, Sequence

from hivedb import HiveDB
from langchain_core.chat_history import BaseChatMessageHistory
from langchain_core.messages import BaseMessage, message_to_dict, messages_from_dict

from ._common import UNIT


class HiveDBChatMessageHistory(BaseChatMessageHistory):
    """Mensajes de una sesión, persistidos en una colección de HiveDB (orden de inserción)."""

    def __init__(
        self, db: HiveDB, session_id: str, *, collection_name: str = "lc_chat_messages"
    ) -> None:
        if UNIT in session_id:
            raise ValueError("session_id no puede contener el carácter U+001F")
        self._collection = db.collection(collection_name)
        self.session_id = session_id
        self._prefix = f"{session_id}{UNIT}"
        self._last = 0

    @property
    def messages(self) -> List[BaseMessage]:  # type: ignore[override]
        entries = self._collection.scan({"prefix": self._prefix})
        return messages_from_dict([e.doc for e in entries])

    def add_messages(self, messages: Sequence[BaseMessage]) -> None:
        for message in messages:
            # Ordena por instante de inserción, estrictamente creciente aunque el reloj sea grueso
            # (Windows); el sufijo aleatorio evita colisiones entre procesos.
            self._last = max(time.time_ns(), self._last + 1)
            id = f"{self._prefix}{self._last:020d}-{uuid.uuid4().hex[:8]}"
            self._collection.put(id, message_to_dict(message))

    def clear(self) -> None:
        for entry in self._collection.scan({"prefix": self._prefix}):
            self._collection.delete(entry.id)
