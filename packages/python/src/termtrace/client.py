"""Synchronous driver transport and domain namespaces."""

import os
import subprocess
import threading
import uuid
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import IO, Literal, Self, TypeVar

import msgspec

from termtrace.atoms import KeyboardAtom, encode_input_sequence
from termtrace.binary import resolve_binary
from termtrace.duration import seconds_to_milliseconds, seconds_to_nanoseconds
from termtrace.environment import (
    EnvironmentOptions as EnvironmentOptions,
    encode_environment,
)
from termtrace.errors import TermtraceOperationError, TermtraceProtocolError
from termtrace.models import (
    Artifact,
    BundleResult,
    Completion,
    ControlResult,
    ControlStableSnapshotData,
    DiagnosticReport,
    DriverArtifactData,
    DriverAttachedData,
    DriverBundleData,
    DriverCompletionData,
    DriverControlData,
    DriverDetachedData,
    DriverDiagnoseData,
    DriverDiffData,
    DriverDiscardedData,
    DriverEventsData,
    DriverHelloData,
    DriverObservationData,
    DriverOutputWaitData,
    DriverTimelineData,
    DriverVerificationData,
    EventPage,
    Frame,
    JsonObject,
    JsonValue,
    Observation,
    SemanticDiff,
    TimelineEntry,
    VerificationReport,
    WaitOutcome,
    decode_driver_response,
    encode_driver_request,
)


ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)


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
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        request_id: str | None = None,
    ) -> ResponseDataT:
        correlation_id = request_id or str(uuid.uuid4())
        payload = encode_driver_request(method, correlation_id)
        with self._lock:
            self._stdin.write(payload + b"\n")
            self._stdin.flush()
            line = self._stdout.readline()
        if not line:
            raise TermtraceProtocolError("driver closed before its correlated response")
        return decode_driver_response(line, correlation_id, response_type)

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
        self._transport.request({"method": "hello"}, DriverHelloData)

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
                "environment": encode_environment(environment),
                "resource_limits": {
                    "output_bytes": output_byte_budget,
                },
            },
            DriverAttachedData,
        )
        return Session(
            self._transport,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=True,
            evidence_policy=evidence,
            completed=data.completed,
        )

    def attach(self, session: str | Path) -> "Session":
        data = self._transport.request(
            {"method": "attach", "session": str(session)}, DriverAttachedData
        )
        return Session(
            self._transport,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=False,
            evidence_policy="manual",
            completed=data.completed,
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
        command: JsonObject,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        response = self._transport.request(
            {
                "method": "control",
                "handle": self.handle,
                "command": command,
                "expected_presented_state_hash": if_state,
            },
            DriverControlData,
            request_id,
        )
        return response.data

    def request(
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        *,
        request_id: str | None = None,
    ) -> ResponseDataT:
        return self._transport.request(
            {**method, "handle": self.handle}, response_type, request_id=request_id
        )

    def close(self) -> None:
        if self._closed:
            return
        if self._owned and self._evidence_policy != "manual":
            self._ensure_completed()
        self._transport.request(
            {"method": "detach", "handle": self.handle}, DriverDetachedData
        )
        self._closed = True

    def finish(self, *, passed: bool) -> None:
        """Complete owned work and apply its explicit retention policy."""

        if self._closed:
            return
        if not self._owned:
            raise ValueError(
                "finish is valid only for sessions launched by this client"
            )
        self._ensure_completed()
        if passed and self._evidence_policy == "on-failure":
            self._transport.request(
                {"method": "discard_completed", "handle": self.handle},
                DriverDiscardedData,
            )
            self._closed = True
            return
        self._transport.request(
            {"method": "detach", "handle": self.handle}, DriverDetachedData
        )
        self._closed = True

    def _ensure_completed(self) -> None:
        if self._completed:
            return
        try:
            self.stop()
        except TermtraceOperationError:
            pass
        self.complete(timeout_ms=10_000)

    def completion(self) -> Completion:
        result = self._transport.request(
            {"method": "completion", "handle": self.handle}, DriverCompletionData
        )
        self._completed = True
        return result.completion

    def stop(
        self, *, request_id: str | None = None, if_state: str | None = None
    ) -> ControlResult:
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
    ) -> ControlResult:
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

    def complete(self, *, timeout_ms: int) -> Completion:
        result = self._transport.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms},
            DriverCompletionData,
        )
        self._completed = True
        return result.completion

    def wait_for_output(self, data: str, *, timeout_ms: int) -> WaitOutcome:
        result = self._transport.request(
            {
                "method": "wait_output",
                "handle": self.handle,
                "data": data,
                "timeout_ms": timeout_ms,
            },
            DriverOutputWaitData,
        )
        return result.outcome

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

    def snapshot(self) -> ControlResult:
        return self._session.control({"command": "snapshot"})

    def text(self) -> str:
        return self.frame().viewport_text

    def frame(self, point: JsonObject | None = None) -> Frame:
        data = self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": False,
            },
            DriverObservationData,
        )
        return data.observation.frame

    def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
        return self.wait_for(
            {"predicate": "contains", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for(
        self,
        predicate: JsonObject,
        *,
        stable_for: float = 0.0,
        timeout: float = 10.0,
    ) -> ControlResult:
        return self._session.control(
            {
                "command": "wait_screen",
                "predicate": predicate,
                "stable_for_ns": seconds_to_nanoseconds(stable_for),
                "timeout_ns": seconds_to_nanoseconds(timeout),
            }
        )

    def wait_for_absence(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
        return self.wait_for(
            {"predicate": "absent", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for_regex(
        self, pattern: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
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
    ) -> ControlStableSnapshotData:
        """Atomically return the exact state that satisfied or ended the idle wait."""
        response = self._session.control(
            {
                "command": "capture_stable",
                "stable_for_ns": seconds_to_nanoseconds(stable_for),
                "timeout_ns": seconds_to_nanoseconds(timeout),
            }
        )
        if not isinstance(response, ControlStableSnapshotData):
            raise TermtraceProtocolError("stable capture response has an invalid kind")
        if response.quality != "stable" and not allow_incomplete:
            raise TermtraceProtocolError(
                f"stable capture ended with {response.quality}; set allow_incomplete to inspect it"
            )
        return response

    def screenshot(
        self,
        destination: str | Path,
        point: JsonObject | None = None,
    ) -> Artifact:
        return self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "semantic",
                "viewport": None,
            },
            DriverArtifactData,
        )


class Keyboard:
    def __init__(self, session: Session) -> None:
        self._session = session

    def press(
        self, key: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> ControlResult:
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
    ) -> ControlResult:
        import base64

        command: JsonObject = {
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
    ) -> ControlResult:
        import base64

        raw = data.encode() if isinstance(data, str) else bytes(data)
        return self._session.control(
            {"command": "paste", "data": base64.b64encode(raw).decode()},
            request_id=request_id,
            if_state=if_state,
        )

    def sequence(
        self,
        atoms: Sequence[KeyboardAtom],
        *,
        pace_ms: int = 0,
        verbose: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        """Apply a complete atom list under one state guard and request ID."""

        sequence_id = request_id or str(uuid.uuid4())
        return self._session.control(
            encode_input_sequence(
                atoms,
                pace_ms=pace_ms,
                verbose=verbose,
                sequence_id=sequence_id,
            ),
            request_id=sequence_id,
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
    ) -> ControlResult:
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
    ) -> ControlResult:
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
    ) -> ControlResult:
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
    ) -> ControlResult:
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
    ) -> ControlResult:
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
        point: JsonObject | None = None,
        *,
        include_hidden: bool = False,
    ) -> Observation:
        result = self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": include_hidden,
            },
            DriverObservationData,
        )
        return result.observation

    def timeline(
        self, *, all_states: bool = False, controls: bool = False
    ) -> list[TimelineEntry]:
        result = self._session.request(
            {
                "method": "timeline",
                "all_states": all_states,
                "controls": controls,
            },
            DriverTimelineData,
        )
        return result.entries

    def diff(self, from_point: JsonObject, to_point: JsonObject) -> SemanticDiff:
        result = self._session.request(
            {"method": "diff", "from": from_point, "to": to_point},
            DriverDiffData,
        )
        return result.difference

    def diagnose(
        self,
        checks: Sequence[str] = (),
        *,
        broad_change_percent: int = 50,
    ) -> DiagnosticReport:
        result = self._session.request(
            {
                "method": "diagnose",
                "checks": list(checks),
                "broad_change_percent": broad_change_percent,
            },
            DriverDiagnoseData,
        )
        return result.report


class Events:
    def __init__(self, session: Session) -> None:
        self._session = session

    def page(
        self,
        *,
        from_event_seq: int = 0,
        limit: int = 256,
        metadata_only: bool = False,
    ) -> EventPage:
        return self._session.request(
            {
                "method": "events",
                "from_event_seq": from_event_seq,
                "limit": limit,
                "metadata_only": metadata_only,
            },
            DriverEventsData,
        )


class Native:
    def __init__(self, session: Session) -> None:
        self._session = session

    def screenshot(
        self,
        destination: str | Path,
        *,
        point: JsonObject | None = None,
        viewport: tuple[int, int] | None = None,
    ) -> Artifact:
        target: JsonValue = (
            None if viewport is None else {"width": viewport[0], "height": viewport[1]}
        )
        return self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "native",
                "viewport": target,
            },
            DriverArtifactData,
        )


class Evidence:
    def __init__(self, session: Session) -> None:
        self._session = session

    def completion(self) -> Completion:
        return self._session.completion()

    def complete(self, *, timeout: float = 10.0) -> Completion:
        return self._session.complete(timeout_ms=seconds_to_milliseconds(timeout))

    def verify(
        self, level: Literal["structure", "content", "durability_record"] = "content"
    ) -> VerificationReport:
        result = self._session.request(
            {"method": "verify", "level": level}, DriverVerificationData
        )
        return result.report

    def bundle(
        self,
        destination: str | Path,
        profile: Literal["screen", "debug", "forensic"] = "screen",
    ) -> BundleResult:
        return self._session.request(
            {
                "method": "bundle",
                "destination": str(destination),
                "profile": profile,
            },
            DriverBundleData,
        )


class Output:
    def __init__(self, session: Session) -> None:
        self._session = session

    def wait_for_bytes(
        self, data: bytes | memoryview, *, timeout: float = 10.0
    ) -> WaitOutcome:
        import base64

        return self._session.wait_for_output(
            base64.b64encode(bytes(data)).decode(),
            timeout_ms=seconds_to_milliseconds(timeout),
        )

    def wait_for_text(self, text: str, *, timeout: float = 10.0) -> WaitOutcome:
        return self.wait_for_bytes(text.encode(), timeout=timeout)
