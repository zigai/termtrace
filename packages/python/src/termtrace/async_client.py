"""Native asyncio transport with explicit subprocess ownership."""

import asyncio
import os
import uuid
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import Literal, Self, TypeVar

import msgspec

from termtrace.binary import resolve_binary
from termtrace.duration import seconds_to_milliseconds, seconds_to_nanoseconds
from termtrace.environment import EnvironmentOptions, encode_environment
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
        cls,
        *,
        binary_path: str | Path | None = None,
        evidence_root: str | Path | None = None,
    ) -> "AsyncTerminal":
        binary = resolve_binary(binary_path)
        process = await asyncio.create_subprocess_exec(
            str(binary),
            "driver",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        terminal = cls(process, None if evidence_root is None else Path(evidence_root))
        await terminal.request({"method": "hello"}, DriverHelloData)
        return terminal

    async def launch(
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
                "environment": encode_environment(environment),
                "resource_limits": {
                    "output_bytes": output_byte_budget,
                },
            },
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
        data = await self.request(
            {"method": "attach", "session": str(session)}, DriverAttachedData
        )
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
    ) -> ResponseDataT:
        correlation_id = request_id or str(uuid.uuid4())
        payload = encode_driver_request(method, correlation_id)
        async with self._lock:
            self._stdin.write(payload + b"\n")
            await self._stdin.drain()
            line = await self._stdout.readline()
        if not line:
            raise TermtraceProtocolError("driver closed before its correlated response")
        return decode_driver_response(line, correlation_id, response_type)

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
        self,
        terminal: AsyncTerminal,
        handle: str,
        session_id: str,
        canonical_session: str,
        *,
        owned: bool,
        evidence_policy: Literal["always", "on-failure", "manual"],
        completed: bool,
    ) -> None:
        self._terminal = terminal
        self.handle = handle
        self.session_id = session_id
        self.canonical_session = Path(canonical_session)
        self._owned = owned
        self._evidence_policy = evidence_policy
        self._completed = completed
        self._closed = False
        self.screen = AsyncScreen(self)
        self.keyboard = AsyncKeyboard(self)
        self.output = AsyncOutput(self)
        self.pointer = AsyncPointer(self)
        self.semantic = AsyncSemantic(self)
        self.events = AsyncEvents(self)
        self.native = AsyncNative(self)
        self.evidence = AsyncEvidence(self)

    async def control(
        self,
        command: JsonObject,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        response = await self._terminal.request(
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

    async def completion(self) -> Completion:
        result = await self._terminal.request(
            {"method": "completion", "handle": self.handle}, DriverCompletionData
        )
        self._completed = True
        return result.completion

    async def request(
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        *,
        request_id: str | None = None,
    ) -> ResponseDataT:
        return await self._terminal.request(
            {**method, "handle": self.handle}, response_type, request_id=request_id
        )

    async def stop(
        self, *, request_id: str | None = None, if_state: str | None = None
    ) -> ControlResult:
        return await self.control(
            {"command": "stop"}, request_id=request_id, if_state=if_state
        )

    async def resize(
        self,
        cols: int,
        rows: int,
        *,
        pixel_width: int | None = None,
        pixel_height: int | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return await self.control(
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

    async def complete(self, *, timeout_ms: int) -> Completion:
        result = await self._terminal.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms},
            DriverCompletionData,
        )
        self._completed = True
        return result.completion

    async def wait_for_output(self, data: str, *, timeout_ms: int) -> WaitOutcome:
        result = await self._terminal.request(
            {
                "method": "wait_output",
                "handle": self.handle,
                "data": data,
                "timeout_ms": timeout_ms,
            },
            DriverOutputWaitData,
        )
        return result.outcome

    async def close(self) -> None:
        if self._closed:
            return
        if self._owned and self._evidence_policy != "manual":
            await self._ensure_completed()
        await self._terminal.request(
            {"method": "detach", "handle": self.handle}, DriverDetachedData
        )
        self._closed = True

    async def finish(self, *, passed: bool) -> None:
        """Complete owned work and apply its explicit retention policy."""

        if self._closed:
            return
        if not self._owned:
            raise ValueError(
                "finish is valid only for sessions launched by this client"
            )
        await self._ensure_completed()
        if passed and self._evidence_policy == "on-failure":
            await self._terminal.request(
                {"method": "discard_completed", "handle": self.handle},
                DriverDiscardedData,
            )
            self._closed = True
            return
        await self._terminal.request(
            {"method": "detach", "handle": self.handle}, DriverDetachedData
        )
        self._closed = True

    async def _ensure_completed(self) -> None:
        if self._completed:
            return
        try:
            await self.stop()
        except TermtraceOperationError:
            pass
        await self.complete(timeout_ms=10_000)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        if self._owned and self._evidence_policy != "manual":
            await self.finish(passed=error_type is None)
        else:
            await self.close()


class AsyncScreen:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def snapshot(self) -> ControlResult:
        return await self._session.control({"command": "snapshot"})

    async def text(self) -> str:
        return (await self.frame()).viewport_text

    async def frame(self, point: JsonObject | None = None) -> Frame:
        data = await self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": False,
            },
            DriverObservationData,
        )
        return data.observation.frame

    async def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
        return await self.wait_for(
            {"predicate": "contains", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def wait_for(
        self,
        predicate: JsonObject,
        *,
        stable_for: float = 0.0,
        timeout: float = 10.0,
    ) -> ControlResult:
        return await self._session.control(
            {
                "command": "wait_screen",
                "predicate": predicate,
                "stable_for_ns": seconds_to_nanoseconds(stable_for),
                "timeout_ns": seconds_to_nanoseconds(timeout),
            }
        )

    async def wait_for_absence(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
        return await self.wait_for(
            {"predicate": "absent", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def wait_for_regex(
        self, pattern: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ControlResult:
        return await self.wait_for(
            {"predicate": "matches_regex", "pattern": pattern},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def wait_for_idle(
        self,
        *,
        stable_for: float = 0.1,
        timeout: float = 10.0,
        allow_incomplete: bool = False,
    ) -> ControlStableSnapshotData:
        """Atomically return the exact state that satisfied or ended the idle wait."""
        response = await self._session.control(
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

    async def screenshot(
        self,
        destination: str | Path,
        point: JsonObject | None = None,
    ) -> Artifact:
        return await self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "semantic",
                "viewport": None,
            },
            DriverArtifactData,
        )


class AsyncKeyboard:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def press(
        self, key: str, *, request_id: str | None = None, if_state: str | None = None
    ) -> ControlResult:
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
    ) -> ControlResult:
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

    async def paste(
        self,
        data: str | bytes | memoryview,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        import base64

        raw = data.encode() if isinstance(data, str) else bytes(data)
        return await self._session.control(
            {"command": "paste", "data": base64.b64encode(raw).decode()},
            request_id=request_id,
            if_state=if_state,
        )


class AsyncPointer:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def click(
        self,
        column: int,
        row: int,
        *,
        button: str = "left",
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return await self._session.control(
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

    async def focus(
        self,
        focused: bool,
        *,
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return await self._session.control(
            {"command": "focus", "focused": focused, "strict": strict},
            request_id=request_id,
            if_state=if_state,
        )

    async def multi_click(
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
        return await self._session.control(
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

    async def drag(
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
        return await self._session.control(
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

    async def scroll(
        self,
        column: int,
        row: int,
        lines: int,
        *,
        strict: bool = False,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return await self._session.control(
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


class AsyncSemantic:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def inspect(
        self,
        point: JsonObject | None = None,
        *,
        include_hidden: bool = False,
    ) -> Observation:
        result = await self._session.request(
            {
                "method": "observe",
                "point": point or {"selector": "latest_presented"},
                "include_hidden": include_hidden,
            },
            DriverObservationData,
        )
        return result.observation

    async def timeline(
        self, *, all_states: bool = False, controls: bool = False
    ) -> list[TimelineEntry]:
        result = await self._session.request(
            {
                "method": "timeline",
                "all_states": all_states,
                "controls": controls,
            },
            DriverTimelineData,
        )
        return result.entries

    async def diff(self, from_point: JsonObject, to_point: JsonObject) -> SemanticDiff:
        result = await self._session.request(
            {"method": "diff", "from": from_point, "to": to_point},
            DriverDiffData,
        )
        return result.difference

    async def diagnose(
        self,
        checks: Sequence[str] = (),
        *,
        broad_change_percent: int = 50,
    ) -> DiagnosticReport:
        result = await self._session.request(
            {
                "method": "diagnose",
                "checks": list(checks),
                "broad_change_percent": broad_change_percent,
            },
            DriverDiagnoseData,
        )
        return result.report


class AsyncEvents:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def page(
        self,
        *,
        from_event_seq: int = 0,
        limit: int = 256,
        metadata_only: bool = False,
    ) -> EventPage:
        return await self._session.request(
            {
                "method": "events",
                "from_event_seq": from_event_seq,
                "limit": limit,
                "metadata_only": metadata_only,
            },
            DriverEventsData,
        )


class AsyncNative:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def screenshot(
        self,
        destination: str | Path,
        *,
        point: JsonObject | None = None,
        viewport: tuple[int, int] | None = None,
    ) -> Artifact:
        target: JsonValue = (
            None if viewport is None else {"width": viewport[0], "height": viewport[1]}
        )
        return await self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "native",
                "viewport": target,
            },
            DriverArtifactData,
        )


class AsyncOutput:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def wait_for_bytes(
        self, data: bytes | memoryview, *, timeout: float = 10.0
    ) -> WaitOutcome:
        import base64

        return await self._session.wait_for_output(
            base64.b64encode(bytes(data)).decode(),
            timeout_ms=seconds_to_milliseconds(timeout),
        )

    async def wait_for_text(self, text: str, *, timeout: float = 10.0) -> WaitOutcome:
        return await self.wait_for_bytes(text.encode(), timeout=timeout)


class AsyncEvidence:
    def __init__(self, session: AsyncSession) -> None:
        self._session = session

    async def complete(self, *, timeout: float = 10.0) -> Completion:
        return await self._session.complete(
            timeout_ms=seconds_to_milliseconds(timeout)
        )

    async def verify(
        self, level: Literal["structure", "content", "durability_record"] = "content"
    ) -> VerificationReport:
        result = await self._session.request(
            {"method": "verify", "level": level}, DriverVerificationData
        )
        return result.report

    async def bundle(
        self,
        destination: str | Path,
        profile: Literal["screen", "debug", "forensic"] = "screen",
    ) -> BundleResult:
        return await self._session.request(
            {
                "method": "bundle",
                "destination": str(destination),
                "profile": profile,
            },
            DriverBundleData,
        )
