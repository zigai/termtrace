"""Async driver process ownership and correlated transport."""

import asyncio
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import TYPE_CHECKING, Literal, Self, TypeVar

import msgspec

from termtrace.environment import EnvironmentOptions
from termtrace.errors import ProtocolMismatchError, RequestTimeoutError
from termtrace.transport.binary import resolve_binary
from termtrace.transport.protocol import (
    DriverAttachedData,
    DriverHelloData,
    JsonObject,
    decode_driver_response,
)
from termtrace.transport.requests import attach_request, correlated_request, launch_request

if TYPE_CHECKING:
    from termtrace.session.async_ import AsyncSession


DRIVER_RESPONSE_STREAM_LIMIT = 4 * 1024 * 1024 + 1

ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)


class AsyncTerminal:
    """Async context manager owning one driver process and stream pair."""

    def __init__(self, process: asyncio.subprocess.Process, evidence_root: Path | None) -> None:
        if process.stdin is None or process.stdout is None:
            raise ProtocolMismatchError("driver pipes were not created")
        self._process = process
        self._stdin = process.stdin
        self._stdout = process.stdout
        self._lock = asyncio.Lock()
        self._evidence_root = evidence_root
        self._closed = False

    @classmethod
    async def create(
        cls,
        *,
        binary_path: str | Path | None = None,
        evidence_root: str | Path | None = None,
    ) -> "AsyncTerminal":
        binary = resolve_binary(binary_path)
        process = await asyncio.create_subprocess_exec(
            str(binary),
            "agent",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
            limit=DRIVER_RESPONSE_STREAM_LIMIT,
        )
        terminal = cls(process, None if evidence_root is None else Path(evidence_root))
        await terminal.request({"method": "hello"}, DriverHelloData)
        return terminal

    async def launch(
        self,
        command: Sequence[str | Path],
        *,
        evidence_root: str | Path | None = None,
        cols: int = 160,
        rows: int = 45,
        working_directory: str | Path | None = None,
        evidence: Literal["always", "on-failure", "manual"] = "always",
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
    ) -> "AsyncSession":

        from termtrace.session.async_ import AsyncSession

        data = await self.request(
            launch_request(
                command,
                default_evidence_root=self._evidence_root,
                evidence_root=evidence_root,
                owner="AsyncTerminal",
                cols=cols,
                rows=rows,
                working_directory=working_directory,
                environment=environment,
                output_byte_budget=output_byte_budget,
            ),
            DriverAttachedData,
        )
        return AsyncSession(
            self,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=True,
            evidence_policy=evidence,
            completed=data.completed,
        )

    async def attach(self, session: str | Path) -> "AsyncSession":
        from termtrace.session.async_ import AsyncSession

        data = await self.request(attach_request(session), DriverAttachedData)
        return AsyncSession(
            self,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=False,
            evidence_policy="manual",
            completed=data.completed,
        )

    async def request(
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        request_id: str | None = None,
        request_timeout: float | None = None,
    ) -> ResponseDataT:
        if self._closed:
            raise ProtocolMismatchError("driver is closed")
        correlation_id, payload = correlated_request(method, request_id, request_timeout)
        try:
            async with asyncio.timeout(request_timeout):
                async with self._lock:
                    if self._closed:
                        raise ProtocolMismatchError("driver is closed")
                    self._stdin.write(payload + b"\n")
                    await self._stdin.drain()
                    line = await self._stdout.readline()
        except TimeoutError:
            self._abort_transport()
            assert request_timeout is not None
            raise RequestTimeoutError(request_timeout) from None
        except asyncio.CancelledError:
            self._abort_transport()
            raise
        if not line:
            raise ProtocolMismatchError("driver closed before its correlated response")
        return decode_driver_response(line, correlation_id, response_type)

    def _abort_transport(self) -> None:
        self._closed = True
        self._stdin.close()
        if self._process.returncode is not None:
            return
        try:
            self._process.terminate()
        except ProcessLookupError:
            return

    async def close(self) -> None:
        self._closed = True
        self._stdin.close()
        try:
            await self._stdin.wait_closed()
        except (BrokenPipeError, ConnectionResetError):
            pass
        if self._process.returncode is not None:
            return
        try:
            async with asyncio.timeout(5):
                await self._process.wait()
        except TimeoutError:
            try:
                self._process.terminate()
            except ProcessLookupError:
                return
            await self._process.wait()

    @property
    def closed(self) -> bool:
        return self._closed

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()
