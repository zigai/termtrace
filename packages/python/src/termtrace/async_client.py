"""Native asyncio transport with explicit subprocess ownership."""

import asyncio
import json
import os
import uuid
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import Self

from termtrace.errors import TermtraceOperationError, TermtraceProtocolError
from termtrace.models import DriverResponse, JsonValue, parse_error


class AsyncTerminal:
    """Async context manager owning one driver process and stream pair."""

    def __init__(
        self, process: asyncio.subprocess.Process, evidence_root: Path | None
    ) -> None:
        if process.stdin is None or process.stdout is None:
            raise TermtraceProtocolError("driver pipes were not created")
        self._process = process
        self._stdin = process.stdin
        self._stdout = process.stdout
        self._lock = asyncio.Lock()
        self._evidence_root = evidence_root

    @classmethod
    async def create(
        cls, *, binary_path: str | Path, evidence_root: str | Path | None = None
    ) -> "AsyncTerminal":
        process = await asyncio.create_subprocess_exec(
            str(binary_path),
            "driver",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        terminal = cls(process, None if evidence_root is None else Path(evidence_root))
        await terminal.request({"method": "hello"})
        return terminal

    async def launch(
        self,
        command: Sequence[str | Path],
        *,
        evidence_root: str | Path | None = None,
        cols: int = 80,
        rows: int = 24,
        working_directory: str | Path | None = None,
    ) -> "AsyncSession":
        import base64

        root = Path(evidence_root) if evidence_root is not None else self._evidence_root
        if root is None:
            raise ValueError(
                "launch requires evidence_root on AsyncTerminal or this call"
            )
        encoded: list[JsonValue] = [
            base64.b64encode(os.fsencode(argument)).decode() for argument in command
        ]
        data = await self.request(
            {
                "method": "launch",
                "evidence_root": str(root),
                "command": encoded,
                "cols": cols,
                "rows": rows,
                "working_directory": None
                if working_directory is None
                else str(working_directory),
            }
        )
        return AsyncSession(
            self,
            _required_str(data, "handle"),
            _required_str(data, "canonical_session"),
        )

    async def attach(self, session: str | Path) -> "AsyncSession":
        data = await self.request({"method": "attach", "session": str(session)})
        return AsyncSession(
            self,
            _required_str(data, "handle"),
            _required_str(data, "canonical_session"),
        )

    async def request(
        self,
        method: dict[str, JsonValue],
        request_id: str | None = None,
    ) -> dict[str, JsonValue]:
        correlation_id = request_id or str(uuid.uuid4())
        payload: dict[str, JsonValue] = {
            "version": 1,
            "request_id": correlation_id,
            "method": method,
        }
        async with self._lock:
            self._stdin.write(
                json.dumps(payload, separators=(",", ":")).encode() + b"\n"
            )
            await self._stdin.drain()
            line = await self._stdout.readline()
        if not line:
            raise TermtraceProtocolError("driver closed before its correlated response")
        try:
            raw: JsonValue = json.loads(line)
        except json.JSONDecodeError as e:
            raise TermtraceProtocolError("driver response is not valid JSON") from e
        failure = parse_error(raw, correlation_id)
        if failure is not None:
            raise TermtraceOperationError(
                _required_str(failure, "message"),
                code=_required_str(failure, "code"),
                retry_safety=_required_str(failure, "retry_safety"),
                uncertain=_required_bool(failure, "uncertain"),
                evidence=_optional_str(failure, "evidence"),
            )
        return DriverResponse.parse(raw, correlation_id).data

    async def close(self) -> None:
        if self._process.returncode is not None:
            return
        self._stdin.close()
        await self._stdin.wait_closed()
        try:
            async with asyncio.timeout(5):
                await self._process.wait()
        except TimeoutError:
            self._process.terminate()
            await self._process.wait()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()


class AsyncSession:
    def __init__(
        self, terminal: AsyncTerminal, handle: str, canonical_session: str
    ) -> None:
        self._terminal = terminal
        self.handle = handle
        self.canonical_session = Path(canonical_session)
        self._closed = False
        self.screen = AsyncScreen(self)
        self.keyboard = AsyncKeyboard(self)
        self.output = AsyncOutput(self)
        self.evidence = AsyncEvidence(self)

    async def control(
        self,
        command: dict[str, JsonValue],
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return await self._terminal.request(
            {
                "method": "control",
                "handle": self.handle,
                "command": command,
                "expected_presented_state_hash": if_state,
            },
            request_id,
        )

    async def completion(self) -> dict[str, JsonValue]:
        return await self._terminal.request(
            {"method": "completion", "handle": self.handle}
        )

    async def stop(
        self, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        return await self.control(
            {"command": "stop"}, request_id=request_id, if_state=if_state
        )

    async def complete(self, *, timeout_ms: int) -> dict[str, JsonValue]:
        return await self._terminal.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms}
        )

    async def wait_for_output(
        self, data: str, *, timeout_ms: int
    ) -> dict[str, JsonValue]:
        return await self._terminal.request(
            {
                "method": "wait_output",
                "handle": self.handle,
                "data": data,
                "timeout_ms": timeout_ms,
            }
        )

    async def close(self) -> None:
        if self._closed:
            return
        await self._terminal.request({"method": "detach", "handle": self.handle})
        self._closed = True

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()


class AsyncScreen:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def snapshot(self) -> dict[str, JsonValue]:
        return await self._session.control({"command": "snapshot"})

    async def text(self) -> str:
        return _required_str(_control_data(await self.snapshot()), "viewport_text")

    async def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return await self._session.control(
            {
                "command": "wait_screen",
                "predicate": {"predicate": "contains", "text": text},
                "stable_for_ns": _seconds_to_ns(stable_for),
                "timeout_ns": _seconds_to_ns(timeout),
            }
        )


class AsyncKeyboard:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def press(
        self, key: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        return await self._session.control(
            {
                "command": "key",
                "key": key,
                "modifiers": {
                    "shift": False,
                    "control": False,
                    "alt": False,
                    "super_key": False,
                },
            },
            request_id=request_id,
            if_state=if_state,
        )

    async def type(
        self, text: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        import base64

        return await self._session.control(
            {
                "command": "input",
                "data": base64.b64encode(text.encode()).decode(),
                "input_kind": "text",
                "intent": None,
            },
            request_id=request_id,
            if_state=if_state,
        )


class AsyncOutput:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def wait_for_bytes(
        self, data: bytes | memoryview, *, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        import base64

        return await self._session.wait_for_output(
            base64.b64encode(bytes(data)).decode(), timeout_ms=_seconds_to_ms(timeout)
        )

    async def wait_for_text(
        self, text: str, *, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return await self.wait_for_bytes(text.encode(), timeout=timeout)


class AsyncEvidence:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def complete(self, *, timeout: float = 10.0) -> dict[str, JsonValue]:
        return await self._session.complete(timeout_ms=_seconds_to_ms(timeout))


def _control_data(value: dict[str, JsonValue]) -> dict[str, JsonValue]:
    if value.get("kind") != "control":
        raise TermtraceProtocolError("driver control response has an invalid kind")
    data = value.get("data")
    if not isinstance(data, dict):
        raise TermtraceProtocolError("driver control response data must be an object")
    return data


def _seconds_to_ns(value: float) -> int:
    if value < 0:
        raise ValueError("duration must be nonnegative")
    return int(value * 1_000_000_000)


def _seconds_to_ms(value: float) -> int:
    if value < 0:
        raise ValueError("duration must be nonnegative")
    return int(value * 1000)


def _required_str(value: dict[str, JsonValue], field: str) -> str:
    item = value.get(field)
    if not isinstance(item, str):
        raise TermtraceProtocolError(f"driver field {field} must be a string")
    return item


def _optional_str(value: dict[str, JsonValue], field: str) -> str | None:
    item = value.get(field)
    if item is None or isinstance(item, str):
        return item
    raise TermtraceProtocolError(f"driver field {field} must be a string or null")


def _required_bool(value: dict[str, JsonValue], field: str) -> bool:
    item = value.get(field)
    if not isinstance(item, bool):
        raise TermtraceProtocolError(f"driver field {field} must be a boolean")
    return item
