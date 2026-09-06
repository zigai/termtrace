"""Fallible application diagnostics over the session control socket."""

from __future__ import annotations

import os
import time
import uuid
from typing import TYPE_CHECKING, Literal

from termtrace.transport.protocol import JsonObject

if TYPE_CHECKING:
    from termtrace.session.async_ import AsyncSession
    from termtrace.session.sync import Session


class _Emitter:
    def __init__(self) -> None:
        self.process_instance_id = str(uuid.uuid4())
        self._started = time.monotonic_ns()

    def _command(
        self,
        kind: Literal["event", "span_start", "span_end"],
        name: str,
        attributes: JsonObject | None,
        span_id: str | None = None,
        parent_span_id: str | None = None,
    ) -> JsonObject:
        return {
            "command": "application",
            "data": {
                "process_instance_id": self.process_instance_id,
                "process_id": os.getpid(),
                "monotonic_ns": time.monotonic_ns() - self._started,
                "kind": kind,
                "name": name,
                "span_id": span_id,
                "parent_span_id": parent_span_id,
                "attributes": attributes if attributes is not None else {},
            },
        }


class Instrumentation(_Emitter):
    """One emitting process instance; recreate after fork and explicitly end spans."""

    def __init__(self, session: Session) -> None:
        super().__init__()
        self._session = session

    def event(self, name: str, attributes: JsonObject | None = None) -> None:
        self._session.control(self._command("event", name, attributes))

    def start_span(
        self,
        name: str,
        attributes: JsonObject | None = None,
        *,
        parent_span_id: str | None = None,
    ) -> str:
        span_id = str(uuid.uuid4())
        self._session.control(
            self._command("span_start", name, attributes, span_id, parent_span_id)
        )
        return span_id

    def end_span(self, name: str, span_id: str, attributes: JsonObject | None = None) -> None:
        self._session.control(self._command("span_end", name, attributes, span_id))


class AsyncInstrumentation(_Emitter):
    """Awaited application diagnostics with explicit span completion and visible failures."""

    def __init__(self, session: AsyncSession) -> None:
        super().__init__()
        self._session = session

    async def event(self, name: str, attributes: JsonObject | None = None) -> None:
        await self._session.control(self._command("event", name, attributes))

    async def start_span(
        self,
        name: str,
        attributes: JsonObject | None = None,
        *,
        parent_span_id: str | None = None,
    ) -> str:
        span_id = str(uuid.uuid4())
        await self._session.control(
            self._command("span_start", name, attributes, span_id, parent_span_id)
        )
        return span_id

    async def end_span(self, name: str, span_id: str, attributes: JsonObject | None = None) -> None:
        await self._session.control(self._command("span_end", name, attributes, span_id))
