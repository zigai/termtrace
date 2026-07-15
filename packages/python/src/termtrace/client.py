"""Synchronous driver transport and domain namespaces."""

import json
import os
import subprocess
import threading
import uuid
from pathlib import Path
from types import TracebackType
from collections.abc import Sequence
from typing import IO, Self

from termtrace.errors import TermtraceOperationError, TermtraceProtocolError
from termtrace.models import DriverResponse, JsonValue, parse_error


class DriverTransport:
    """Own one driver process and correlated request stream."""

    _process: subprocess.Popen[bytes]
    _stdin: IO[bytes]
    _stdout: IO[bytes]

    def __init__(self, binary_path: str | Path) -> None:
        self._process = subprocess.Popen(
            [str(binary_path), "driver"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if self._process.stdin is None or self._process.stdout is None:
            raise TermtraceProtocolError("driver pipes were not created")
        self._stdin = self._process.stdin
        self._stdout = self._process.stdout
        self._lock = threading.Lock()

    def request(
        self, method: dict[str, JsonValue], request_id: str | None = None
    ) -> dict[str, JsonValue]:
        correlation_id = request_id or str(uuid.uuid4())
        payload: dict[str, JsonValue] = {
            "version": 1,
            "request_id": correlation_id,
            "method": method,
        }
        with self._lock:
            self._stdin.write(
                json.dumps(payload, separators=(",", ":")).encode() + b"\n"
            )
            self._stdin.flush()
            line = self._stdout.readline()
        if not line:
            raise TermtraceProtocolError("driver closed before its correlated response")
        try:
            raw: JsonValue = json.loads(line)
        except json.JSONDecodeError as e:
            raise TermtraceProtocolError("driver response is not valid JSON") from e
        error = parse_error(raw, correlation_id)
        if error is not None:
            raise TermtraceOperationError(
                _required_str(error, "message"),
                code=_required_str(error, "code"),
                retry_safety=_required_str(error, "retry_safety"),
                uncertain=_required_bool(error, "uncertain"),
                evidence=_optional_str(error, "evidence"),
            )
        return DriverResponse.parse(raw, correlation_id).data

    def close(self) -> None:
        if self._process.poll() is not None:
            return
        self._stdin.close()
        try:
            self._process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._process.terminate()
            self._process.wait(timeout=5)


class Terminal:
    """Context-managed synchronous client over one driver process."""

    def __init__(
        self, *, binary_path: str | Path, evidence_root: str | Path | None = None
    ) -> None:
        self._transport = DriverTransport(binary_path)
        self._evidence_root = None if evidence_root is None else Path(evidence_root)
        self._transport.request({"method": "hello"})

    def launch(
        self,
        command: Sequence[str | Path],
        *,
        evidence_root: str | Path | None = None,
        cols: int = 80,
        rows: int = 24,
        working_directory: str | Path | None = None,
    ) -> "Session":
        import base64

        root = Path(evidence_root) if evidence_root is not None else self._evidence_root
        if root is None:
            raise ValueError("launch requires evidence_root on Terminal or this call")
        encoded: list[JsonValue] = [
            base64.b64encode(os.fsencode(argument)).decode() for argument in command
        ]
        data = self._transport.request(
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
        return Session(
            self._transport,
            _required_str(data, "handle"),
            _required_str(data, "canonical_session"),
        )

    def attach(self, session: str | Path) -> "Session":
        data = self._transport.request({"method": "attach", "session": str(session)})
        return Session(
            self._transport,
            _required_str(data, "handle"),
            _required_str(data, "canonical_session"),
        )

    def close(self) -> None:
        self._transport.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()


class Session:
    def __init__(
        self, transport: DriverTransport, handle: str, canonical_session: str
    ) -> None:
        self._transport = transport
        self.handle = handle
        self.canonical_session = Path(canonical_session)
        self.screen = Screen(self)
        self.keyboard = Keyboard(self)
        self.output = Output(self)
        self.evidence = Evidence(self)
        self._closed = False

    def control(
        self,
        command: dict[str, JsonValue],
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._transport.request(
            {
                "method": "control",
                "handle": self.handle,
                "command": command,
                "expected_presented_state_hash": if_state,
            },
            request_id,
        )

    def close(self) -> None:
        if self._closed:
            return
        self._transport.request({"method": "detach", "handle": self.handle})
        self._closed = True

    def completion(self) -> dict[str, JsonValue]:
        return self._transport.request({"method": "completion", "handle": self.handle})

    def stop(
        self, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        return self.control(
            {"command": "stop"}, request_id=request_id, if_state=if_state
        )

    def complete(self, *, timeout_ms: int) -> dict[str, JsonValue]:
        return self._transport.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms}
        )

    def wait_for_output(self, data: str, *, timeout_ms: int) -> dict[str, JsonValue]:
        return self._transport.request(
            {
                "method": "wait_output",
                "handle": self.handle,
                "data": data,
                "timeout_ms": timeout_ms,
            }
        )

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()


class Screen:
    def __init__(self, session: Session) -> None:
        self._session = session

    def snapshot(self) -> dict[str, JsonValue]:
        return self._session.control({"command": "snapshot"})

    def text(self) -> str:
        return _required_str(_control_data(self.snapshot()), "viewport_text")

    def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "wait_screen",
                "predicate": {"predicate": "contains", "text": text},
                "stable_for_ns": _seconds_to_ns(stable_for),
                "timeout_ns": _seconds_to_ns(timeout),
            }
        )


class Keyboard:
    def __init__(self, session: Session) -> None:
        self._session = session

    def press(
        self, key: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        return self._session.control(
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

    def type(
        self, text: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        import base64

        command: dict[str, JsonValue] = {
            "command": "input",
            "data": base64.b64encode(text.encode()).decode(),
            "input_kind": "text",
            "intent": None,
        }
        return self._session.control(command, request_id=request_id, if_state=if_state)


class Evidence:
    def __init__(self, session: Session) -> None:
        self._session = session

    def completion(self) -> dict[str, JsonValue]:
        return self._session.completion()

    def complete(self, *, timeout: float = 10.0) -> dict[str, JsonValue]:
        return self._session.complete(timeout_ms=_seconds_to_ms(timeout))


class Output:
    def __init__(self, session: Session) -> None:
        self._session = session

    def wait_for_bytes(
        self, data: bytes | memoryview, *, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        import base64

        return self._session.wait_for_output(
            base64.b64encode(bytes(data)).decode(), timeout_ms=_seconds_to_ms(timeout)
        )

    def wait_for_text(
        self, text: str, *, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return self.wait_for_bytes(text.encode(), timeout=timeout)


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
