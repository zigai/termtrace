"""Synchronous driver transport and domain namespaces."""

import json
import os
import subprocess
import threading
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from types import TracebackType
from collections.abc import Mapping, Sequence
from typing import IO, Literal, Self

from termtrace.binary import resolve_binary
from termtrace.errors import TermtraceOperationError, TermtraceProtocolError
from termtrace.models import DriverResponse, JsonValue, parse_error


@dataclass(frozen=True, slots=True)
class EnvironmentOptions:
    """Deterministic child-environment policy transported only through driver stdin."""

    inherit_env: bool = True
    set: Mapping[str, str | bytes] = field(default_factory=dict)
    copy: Sequence[str] = ()
    unset: Sequence[str] = ()
    color: Literal["auto", "always", "never"] = "auto"


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
            "version": 2,
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
        self,
        *,
        binary_path: str | Path | None = None,
        evidence_root: str | Path | None = None,
    ) -> None:
        self._transport = DriverTransport(resolve_binary(binary_path))
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
        evidence: Literal["always", "on-failure", "manual"] = "always",
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
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
                "environment": _encode_environment(environment),
                "resource_limits": {
                    "output_bytes": output_byte_budget,
                },
            }
        )
        return Session(
            self._transport,
            _required_str(data, "handle"),
            _required_str(data, "session_id"),
            _required_str(data, "canonical_session"),
            owned=True,
            evidence_policy=evidence,
            completed=_required_bool(data, "completed"),
        )

    def attach(self, session: str | Path) -> "Session":
        data = self._transport.request({"method": "attach", "session": str(session)})
        return Session(
            self._transport,
            _required_str(data, "handle"),
            _required_str(data, "session_id"),
            _required_str(data, "canonical_session"),
            owned=False,
            evidence_policy="manual",
            completed=_required_bool(data, "completed"),
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
        self,
        transport: DriverTransport,
        handle: str,
        session_id: str,
        canonical_session: str,
        *,
        owned: bool,
        evidence_policy: Literal["always", "on-failure", "manual"],
        completed: bool,
    ) -> None:
        self._transport = transport
        self.handle = handle
        self.session_id = session_id
        self.canonical_session = Path(canonical_session)
        self._owned = owned
        self._evidence_policy = evidence_policy
        self._completed = completed
        self.screen = Screen(self)
        self.keyboard = Keyboard(self)
        self.output = Output(self)
        self.pointer = Pointer(self)
        self.semantic = Semantic(self)
        self.events = Events(self)
        self.native = Native(self)
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

    def request(
        self, method: dict[str, JsonValue], *, request_id: str | None = None
    ) -> dict[str, JsonValue]:
        return self._transport.request(
            {**method, "handle": self.handle}, request_id=request_id
        )

    def close(self) -> None:
        if self._closed:
            return
        if self._owned and self._evidence_policy != "manual":
            self._ensure_completed()
        self._transport.request({"method": "detach", "handle": self.handle})
        self._closed = True

    def finish(self, *, passed: bool) -> None:
        """Complete owned work and apply its explicit retention policy."""

        if self._closed:
            return
        if not self._owned:
            raise ValueError("finish is valid only for sessions launched by this client")
        self._ensure_completed()
        if passed and self._evidence_policy == "on-failure":
            self._transport.request(
                {"method": "discard_completed", "handle": self.handle}
            )
            self._closed = True
            return
        self._transport.request({"method": "detach", "handle": self.handle})
        self._closed = True

    def _ensure_completed(self) -> None:
        if self._completed:
            return
        try:
            self.stop()
        except TermtraceOperationError:
            pass
        self.complete(timeout_ms=10_000)

    def completion(self) -> dict[str, JsonValue]:
        result = self._transport.request({"method": "completion", "handle": self.handle})
        self._completed = True
        return result

    def stop(
        self, *, request_id: str | None = None, if_state: str | None = None
    ) -> dict[str, JsonValue]:
        return self.control(
            {"command": "stop"}, request_id=request_id, if_state=if_state
        )

    def resize(
        self,
        cols: int,
        rows: int,
        *,
        pixel_width: int | None = None,
        pixel_height: int | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self.control(
            {
                "command": "resize",
                "cols": cols,
                "rows": rows,
                "pixel_width": pixel_width if pixel_width is not None else cols * 8,
                "pixel_height": pixel_height if pixel_height is not None else rows * 16,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def complete(self, *, timeout_ms: int) -> dict[str, JsonValue]:
        result = self._transport.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms}
        )
        self._completed = True
        return result

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
        if self._owned and self._evidence_policy != "manual":
            self.finish(passed=error_type is None)
        else:
            self.close()


class Screen:
    def __init__(self, session: Session) -> None:
        self._session = session

    def snapshot(self) -> dict[str, JsonValue]:
        return self._session.control({"command": "snapshot"})

    def text(self) -> str:
        return _required_str(self.frame(), "viewport_text")

    def frame(
        self, point: dict[str, JsonValue] | None = None
    ) -> dict[str, JsonValue]:
        data = self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": False,
            }
        )
        observation = _required_mapping(data, "observation")
        return _required_mapping(observation, "frame")

    def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return self.wait_for(
            {"predicate": "contains", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for(
        self,
        predicate: dict[str, JsonValue],
        *,
        stable_for: float = 0.0,
        timeout: float = 10.0,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "wait_screen",
                "predicate": predicate,
                "stable_for_ns": _seconds_to_ns(stable_for),
                "timeout_ns": _seconds_to_ns(timeout),
            }
        )

    def wait_for_absence(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return self.wait_for(
            {"predicate": "absent", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for_regex(
        self, pattern: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> dict[str, JsonValue]:
        return self.wait_for(
            {"predicate": "matches_regex", "pattern": pattern},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for_idle(
        self,
        *,
        stable_for: float = 0.1,
        timeout: float = 10.0,
        allow_incomplete: bool = False,
    ) -> dict[str, JsonValue]:
        """Atomically return the exact state that satisfied or ended the idle wait."""
        response = self._session.control(
            {
                "command": "capture_stable",
                "stable_for_ns": _seconds_to_ns(stable_for),
                "timeout_ns": _seconds_to_ns(timeout),
            }
        )
        if _required_str(response, "kind") != "control":
            raise TermtraceProtocolError("stable capture response has an invalid wrapper")
        data = _required_mapping(response, "data")
        if _required_str(data, "kind") != "stable_snapshot":
            raise TermtraceProtocolError("stable capture response has an invalid kind")
        quality = _required_str(data, "quality")
        if quality != "stable" and not allow_incomplete:
            raise TermtraceProtocolError(
                f"stable capture ended with {quality}; set allow_incomplete to inspect it"
            )
        return data

    def screenshot(
        self,
        destination: str | Path,
        point: dict[str, JsonValue] | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "semantic",
                "viewport": None,
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

    def paste(
        self,
        data: str | bytes | memoryview,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        import base64

        raw = data.encode() if isinstance(data, str) else bytes(data)
        return self._session.control(
            {"command": "paste", "data": base64.b64encode(raw).decode()},
            request_id=request_id,
            if_state=if_state,
        )


class Pointer:
    def __init__(self, session: Session) -> None:
        self._session = session

    def click(
        self,
        column: int,
        row: int,
        *,
        button: str = "left",
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "pointer",
                "column": column,
                "row": row,
                "operation": {"kind": "click", "button": button},
                "strict": strict,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def scroll(
        self,
        column: int,
        row: int,
        lines: int,
        *,
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "pointer",
                "column": column,
                "row": row,
                "operation": {"kind": "scroll", "lines": lines},
                "strict": strict,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def multi_click(
        self,
        column: int,
        row: int,
        count: int,
        *,
        button: str = "left",
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "pointer",
                "column": column,
                "row": row,
                "operation": {"kind": "multi_click", "button": button, "count": count},
                "strict": strict,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def drag(
        self,
        from_column: int,
        from_row: int,
        to_column: int,
        to_row: int,
        *,
        button: str = "left",
        steps: int = 8,
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {
                "command": "pointer",
                "column": from_column,
                "row": from_row,
                "operation": {
                    "kind": "drag",
                    "button": button,
                    "to_column": to_column,
                    "to_row": to_row,
                    "steps": steps,
                },
                "strict": strict,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def focus(
        self,
        focused: bool,
        *,
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> dict[str, JsonValue]:
        return self._session.control(
            {"command": "focus", "focused": focused, "strict": strict},
            request_id=request_id,
            if_state=if_state,
        )


class Semantic:
    def __init__(self, session: Session) -> None:
        self._session = session

    def inspect(
        self,
        point: dict[str, JsonValue] | None = None,
        *,
        include_hidden: bool = False,
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": include_hidden,
            }
        )

    def timeline(
        self, *, all_states: bool = False, controls: bool = False
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "timeline",
                "all_states": all_states,
                "controls": controls,
            }
        )

    def diff(
        self, from_point: dict[str, JsonValue], to_point: dict[str, JsonValue]
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {"method": "diff", "from": from_point, "to": to_point}
        )

    def diagnose(
        self,
        checks: Sequence[str] = (),
        *,
        broad_change_percent: int = 50,
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "diagnose",
                "checks": list(checks),
                "broad_change_percent": broad_change_percent,
            }
        )


class Events:
    def __init__(self, session: Session) -> None:
        self._session = session

    def page(
        self,
        *,
        from_event_seq: int = 0,
        limit: int = 256,
        metadata_only: bool = False,
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "events",
                "from_event_seq": from_event_seq,
                "limit": limit,
                "metadata_only": metadata_only,
            }
        )


class Native:
    def __init__(self, session: Session) -> None:
        self._session = session

    def screenshot(
        self,
        destination: str | Path,
        *,
        point: dict[str, JsonValue] | None = None,
        viewport: tuple[int, int] | None = None,
    ) -> dict[str, JsonValue]:
        target: JsonValue = (
            None
            if viewport is None
            else {"width": viewport[0], "height": viewport[1]}
        )
        return self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "native",
                "viewport": target,
            }
        )


class Evidence:
    def __init__(self, session: Session) -> None:
        self._session = session

    def completion(self) -> dict[str, JsonValue]:
        return self._session.completion()

    def complete(self, *, timeout: float = 10.0) -> dict[str, JsonValue]:
        return self._session.complete(timeout_ms=_seconds_to_ms(timeout))

    def verify(
        self, level: Literal["structure", "content", "durability_record"] = "content"
    ) -> dict[str, JsonValue]:
        return self._session.request({"method": "verify", "level": level})

    def bundle(
        self,
        destination: str | Path,
        profile: Literal["screen", "debug", "forensic"] = "screen",
    ) -> dict[str, JsonValue]:
        return self._session.request(
            {
                "method": "bundle",
                "destination": str(destination),
                "profile": profile,
            }
        )


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


def _encode_environment(options: EnvironmentOptions | None) -> dict[str, JsonValue]:
    import base64

    selected = options or EnvironmentOptions()
    encoded: dict[str, JsonValue] = {}
    for name, value in selected.set.items():
        raw = value.encode() if isinstance(value, str) else value
        encoded[name] = base64.b64encode(raw).decode()
    return {
        "base": "inherit" if selected.inherit_env else "clear",
        "set": encoded,
        "copy": list(selected.copy),
        "unset": list(selected.unset),
        "color": selected.color,
    }


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


def _required_mapping(
    value: dict[str, JsonValue], field: str
) -> dict[str, JsonValue]:
    item = value.get(field)
    if not isinstance(item, dict):
        raise TermtraceProtocolError(f"driver field {field} must be an object")
    return item
