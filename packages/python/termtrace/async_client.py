"""Native asyncio transport with explicit subprocess ownership."""

import asyncio
import math
import os
import re
import uuid
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import Literal, Self, TypeVar

import msgspec

from termtrace.atoms import InputAtom, KeyboardAtom, encode_atoms, encode_input_sequence
from termtrace.binary import resolve_binary
from termtrace.duration import (
    Duration,
    duration,
    seconds_to_milliseconds,
)
from termtrace.environment import EnvironmentOptions, encode_environment
from termtrace.errors import (
    PostconditionIncompleteError,
    RequestTimeoutError,
    StableCaptureIncompleteError,
    StaleStateError,
    TermtraceOperationError,
    TermtraceProtocolError,
)
from termtrace.frames import FrameRequest, Region, ScreenFrame
from termtrace.locators import (
    AsyncLocator,
    LocatorSpec,
    StyleFilter,
    TextSpec,
)
from termtrace.locators import (
    region as region_spec,
)
from termtrace.locators import (
    row as row_spec,
)
from termtrace.locators import (
    style as style_spec,
)
from termtrace.models import (
    ActionOptions,
    ActionResult,
    Artifact,
    BundleResult,
    Completion,
    ControlResult,
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
    DriverScreenFrameData,
    DriverTimelineData,
    DriverVerificationData,
    EventPage,
    JsonObject,
    JsonValue,
    Observation,
    ScreenWaitResult,
    SemanticDiff,
    StableCapture,
    StableOptions,
    TimelinePage,
    VerificationReport,
    WaitOutcome,
    decode_driver_response,
    encode_driver_request,
    is_incomplete_completion,
)
from termtrace.normalization import (
    encode_frame_request,
    encode_postcondition,
    normalize_action,
    normalize_frame,
    normalize_screen_wait,
    normalize_stable_capture,
)

DRIVER_RESPONSE_STREAM_LIMIT = 4 * 1024 * 1024 + 1

ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)


class AsyncTerminal:
    """Async context manager owning one driver process and stream pair."""

    def __init__(self, process: asyncio.subprocess.Process, evidence_root: Path | None) -> None:
        if process.stdin is None or process.stdout is None:
            raise TermtraceProtocolError("driver pipes were not created")
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
            "tooling",
            "driver",
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
        import base64

        root = Path(evidence_root) if evidence_root is not None else self._evidence_root
        if root is None:
            raise ValueError("launch requires evidence_root on AsyncTerminal or this call")
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
                "working_directory": None if working_directory is None else str(working_directory),
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
        data = await self.request({"method": "attach", "session": str(session)}, DriverAttachedData)
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
            raise TermtraceProtocolError("driver is closed")
        if request_timeout is not None and (
            not math.isfinite(request_timeout) or request_timeout <= 0
        ):
            raise ValueError("request timeout must be finite and greater than zero")
        correlation_id = request_id or str(uuid.uuid4())
        payload = encode_driver_request(method, correlation_id)
        try:
            async with asyncio.timeout(request_timeout):
                async with self._lock:
                    if self._closed:
                        raise TermtraceProtocolError("driver is closed")
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
            raise TermtraceProtocolError("driver closed before its correlated response")
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
        request_timeout: float | None = None,
    ) -> ControlResult:
        try:
            response = await self._terminal.request(
                {
                    "method": "control",
                    "handle": self.handle,
                    "command": command,
                    "expected_presented_state_hash": if_state,
                },
                DriverControlData,
                request_id=request_id,
                request_timeout=request_timeout,
            )
        except StaleStateError as e:
            raise StaleStateError(
                str(e),
                expected_state_hash=if_state,
                current_state_hash=e.current_state_hash,
                retry_safety=e.retry_safety,
                evidence=e.evidence,
            ) from e
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
        request_timeout: float | None = None,
    ) -> ResponseDataT:
        return await self._terminal.request(
            {**method, "handle": self.handle},
            response_type,
            request_id=request_id,
            request_timeout=request_timeout,
        )

    async def act(
        self,
        atoms: Sequence[InputAtom],
        options: ActionOptions = ActionOptions(),
    ) -> ActionResult:
        request_id = options.request_id or str(uuid.uuid4())
        pace = options.pace
        if pace > Duration.seconds(60):
            raise ValueError("action pace must not exceed 60 seconds")
        if pace.nanoseconds * len(atoms) > Duration.seconds(60).nanoseconds:
            raise ValueError("total action pacing must not exceed 60 seconds")
        response = await self.control(
            {
                "command": "act",
                "atoms": encode_atoms(
                    atoms, request_id=request_id, caller_intent=options.caller_intent
                ),
                "pace_ns": pace.nanoseconds,
                "include_atom_outcomes": options.verbose_atom_outcomes,
                "postcondition": encode_postcondition(options.postcondition),
                "projection": encode_frame_request(options.frame),
            },
            request_id=request_id,
            if_state=options.if_state,
        )
        result = normalize_action(response, request_id)
        if is_incomplete_completion(result.postcondition) and not options.allow_incomplete:
            raise PostconditionIncompleteError(result)
        return result

    async def stop(
        self,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
        request_timeout: float | None = None,
    ) -> ControlResult:
        return await self.control(
            {"command": "stop"},
            request_id=request_id,
            if_state=if_state,
            request_timeout=request_timeout,
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
                "pixel_width": pixel_width if pixel_width is not None else cols * 12,
                "pixel_height": pixel_height if pixel_height is not None else rows * 24,
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
            raise ValueError("finish is valid only for sessions launched by this client")
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
        data = await self._session.request(
            {
                "method": "observe",
                "point": {"selector": "latest_presented"},
                "include_hidden": False,
            },
            DriverObservationData,
        )
        return data.observation.frame.viewport_text

    async def frame(
        self,
        options: FrameRequest = FrameRequest(),
        *,
        request_timeout: float | None = None,
    ) -> ScreenFrame:
        data = await self._session.request(
            {
                "method": "screen_frame",
                "point": {"selector": "latest_presented"},
                "include_hidden": False,
                "projection": encode_frame_request(options),
            },
            DriverScreenFrameData,
            request_timeout=request_timeout,
        )
        return normalize_frame(data.frame)

    async def cells(self, region: Region) -> ScreenFrame:
        return await self.frame(FrameRequest("cells", region))

    def locator(self, spec: LocatorSpec) -> AsyncLocator:
        async def capture(request_timeout: float | None) -> ScreenFrame:
            return await self.frame(request_timeout=request_timeout)

        return AsyncLocator(capture, spec)

    def get_by_text(
        self,
        query: str | re.Pattern[str],
        *,
        exact: bool = False,
        cross_soft_wraps: bool = False,
    ) -> AsyncLocator:
        return self.locator(TextSpec(query, exact, cross_soft_wraps))

    def row(self, index: int) -> AsyncLocator:
        return self.locator(row_spec(index))

    def region(self, value: Region) -> AsyncLocator:
        return self.locator(region_spec(value))

    def filter(self, style: StyleFilter) -> AsyncLocator:
        return self.locator(style_spec(style))

    async def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return await self.wait_for(
            {"predicate": "contains", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def wait_for(
        self,
        predicate: JsonObject,
        *,
        stable_for: Duration | int | float = Duration(0),
        timeout: Duration | int | float = Duration.seconds(10),
        frame: FrameRequest = FrameRequest(),
    ) -> ScreenWaitResult:
        stable_value = duration(stable_for)
        timeout_value = duration(timeout)
        if timeout_value.nanoseconds == 0:
            raise ValueError("screen wait timeout must be greater than zero")
        response = await self._session.control(
            {
                "command": "wait_screen",
                "predicate": predicate,
                "stable_for_ns": stable_value.nanoseconds,
                "timeout_ns": timeout_value.nanoseconds,
                "projection": encode_frame_request(frame),
            }
        )
        return normalize_screen_wait(response)

    async def wait_for_absence(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return await self.wait_for(
            {"predicate": "absent", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def wait_for_regex(
        self, pattern: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return await self.wait_for(
            {"predicate": "matches_regex", "pattern": pattern},
            stable_for=stable_for,
            timeout=timeout,
        )

    async def capture_stable(
        self,
        options: StableOptions | None = None,
        *,
        stable_for: Duration | int | float = Duration.milliseconds(100),
        timeout: Duration | int | float = Duration.seconds(10),
        frame: FrameRequest = FrameRequest(),
        allow_incomplete: bool = False,
    ) -> StableCapture:
        selected = options or StableOptions(
            duration(stable_for), duration(timeout), frame, allow_incomplete
        )
        if self._session._completed:
            return StableCapture(
                "stable",
                Duration(0),
                Duration(0),
                selected.stable_for,
                await self.frame(selected.frame),
            )
        response = await self._session.control(
            {
                "command": "capture_stable",
                "stable_for_ns": selected.stable_for.nanoseconds,
                "timeout_ns": selected.timeout.nanoseconds,
                "projection": encode_frame_request(selected.frame),
            }
        )
        result = normalize_stable_capture(response)
        if result.quality != "stable" and not selected.allow_incomplete:
            raise StableCaptureIncompleteError(result)
        return result

    async def wait_for_idle(
        self,
        *,
        stable_for: float = 0.1,
        timeout: float = 10.0,
        allow_incomplete: bool = False,
    ) -> StableCapture:
        return await self.capture_stable(
            stable_for=stable_for, timeout=timeout, allow_incomplete=allow_incomplete
        )

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
        self,
        key: str,
        *,
        caller_intent: str | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
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
                "caller_intent": caller_intent,
            },
            request_id=request_id,
            if_state=if_state,
        )

    async def type(
        self,
        text: str,
        *,
        caller_intent: str | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        import base64

        return await self._session.control(
            {
                "command": "input",
                "data": base64.b64encode(text.encode()).decode(),
                "input_kind": "text",
                "caller_intent": caller_intent,
            },
            request_id=request_id,
            if_state=if_state,
        )

    async def paste(
        self,
        data: str | bytes | memoryview,
        *,
        caller_intent: str | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        import base64

        raw = data.encode() if isinstance(data, str) else bytes(data)
        return await self._session.control(
            {
                "command": "paste",
                "data": base64.b64encode(raw).decode(),
                "caller_intent": caller_intent,
            },
            request_id=request_id,
            if_state=if_state,
        )

    async def sequence(
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
        return await self._session.control(
            encode_input_sequence(
                atoms,
                pace_ms=pace_ms,
                verbose=verbose,
                sequence_id=sequence_id,
            ),
            request_id=sequence_id,
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
        self,
        *,
        all_states: bool = False,
        controls: bool = False,
        offset: int = 0,
        limit: int = 256,
    ) -> TimelinePage:
        return await self._session.request(
            {
                "method": "timeline",
                "all_states": all_states,
                "controls": controls,
                "offset": offset,
                "limit": limit,
            },
            DriverTimelineData,
        )

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

    async def completion(self) -> Completion:
        return await self._session.completion()

    async def complete(self, *, timeout: float = 10.0) -> Completion:
        return await self._session.complete(timeout_ms=seconds_to_milliseconds(timeout))

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
