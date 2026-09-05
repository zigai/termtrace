"""Synchronous session state and terminal action namespaces."""

import re
import uuid
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import Literal, Self, TypeVar

import msgspec

from termtrace.atoms import InputAtom, KeyboardAtom
from termtrace.duration import Duration, duration, seconds_to_milliseconds
from termtrace.errors import (
    StableCaptureIncompleteError,
    StaleStateError,
    TermtraceOperationError,
)
from termtrace.screen.frame import FrameRequest, Region, ScreenFrame
from termtrace.screen.locators import Locator
from termtrace.screen.queries import LocatorSpec, StyleFilter, TextSpec
from termtrace.screen.queries import region as region_spec
from termtrace.screen.queries import row as row_spec
from termtrace.screen.queries import style as style_spec
from termtrace.screen.wire import encode_frame_request, normalize_frame
from termtrace.session.actions import (
    ActionOptions,
    ActionResult,
    ScreenWaitResult,
    StableCapture,
    StableOptions,
)
from termtrace.session.commands import (
    events_page_request,
    evidence_bundle_request,
    evidence_verify_request,
    keyboard_paste_command,
    keyboard_press_command,
    keyboard_sequence_command,
    keyboard_type_command,
    native_screenshot_request,
    output_wait_payload,
    pointer_click_command,
    pointer_drag_command,
    pointer_focus_command,
    pointer_multi_click_command,
    pointer_scroll_command,
    screen_frame_request,
    screen_observe_request,
    screen_wait_command,
)
from termtrace.session.requests import action_request, require_complete_action
from termtrace.session.wire import (
    normalize_action,
    normalize_screen_wait,
    normalize_stable_capture,
)
from termtrace.transport.protocol import (
    Artifact,
    BundleResult,
    Completion,
    ControlResult,
    DiagnosticReport,
    DriverArtifactData,
    DriverBundleData,
    DriverCompletionData,
    DriverControlData,
    DriverDetachedData,
    DriverDiagnoseData,
    DriverDiffData,
    DriverDiscardedData,
    DriverEventsData,
    DriverObservationData,
    DriverOutputWaitData,
    DriverScreenFrameData,
    DriverTimelineData,
    DriverVerificationData,
    EventPage,
    JsonObject,
    Observation,
    SemanticDiff,
    TimelinePage,
    VerificationReport,
    WaitOutcome,
)
from termtrace.transport.sync import DriverTransport

ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)


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
        request_timeout: float | None = None,
    ) -> ControlResult:
        try:
            response = self._transport.request(
                {
                    "method": "control",
                    "handle": self.handle,
                    "command": command,
                    "expected_presented_state_hash": if_state,
                },
                DriverControlData,
                request_id=request_id,
                timeout=request_timeout,
            )
        except StaleStateError as e:
            e.expected_state_hash = if_state
            raise
        return response.data

    def request(
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        *,
        request_id: str | None = None,
        request_timeout: float | None = None,
    ) -> ResponseDataT:
        return self._transport.request(
            {**method, "handle": self.handle},
            response_type,
            request_id=request_id,
            timeout=request_timeout,
        )

    def act(
        self,
        atoms: Sequence[InputAtom],
        options: ActionOptions = ActionOptions(),
    ) -> ActionResult:
        request_id, command = action_request(atoms, options)
        try:
            response = self.control(
                command,
                request_id=request_id,
                if_state=options.if_state,
            )
        except TermtraceOperationError as e:
            if e.request_id is None:
                e.request_id = request_id
            raise
        return require_complete_action(
            normalize_action(response, request_id),
            allow_incomplete=options.allow_incomplete,
        )

    def close(self) -> None:
        if self._closed:
            return
        if self._owned and self._evidence_policy != "manual":
            self._ensure_completed()
        self._transport.request({"method": "detach", "handle": self.handle}, DriverDetachedData)
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
                {"method": "discard_completed", "handle": self.handle},
                DriverDiscardedData,
            )
            self._closed = True
            return
        self._transport.request({"method": "detach", "handle": self.handle}, DriverDetachedData)
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
        self,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
        request_timeout: float | None = None,
    ) -> ControlResult:
        return self.control(
            {"command": "stop"},
            request_id=request_id,
            if_state=if_state,
            request_timeout=request_timeout,
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
                "pixel_width": pixel_width if pixel_width is not None else cols * 12,
                "pixel_height": pixel_height if pixel_height is not None else rows * 24,
            },
            request_id=request_id,
            if_state=if_state,
        )

    def complete(
        self,
        *,
        timeout_ms: int,
        request_timeout: float | None = None,
    ) -> Completion:
        result = self._transport.request(
            {"method": "complete", "handle": self.handle, "timeout_ms": timeout_ms},
            DriverCompletionData,
            timeout=request_timeout,
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
        data = self._session.request(screen_observe_request(), DriverObservationData)
        return data.observation.frame.viewport_text

    def frame(
        self,
        options: FrameRequest = FrameRequest(),
        *,
        request_timeout: float | None = None,
    ) -> ScreenFrame:
        data = self._session.request(
            screen_frame_request(options),
            DriverScreenFrameData,
            request_timeout=request_timeout,
        )
        return normalize_frame(data.frame)

    def cells(self, region: Region) -> ScreenFrame:
        return self.frame(FrameRequest("cells", region))

    def locator(self, spec: LocatorSpec) -> Locator:
        return Locator(
            lambda request_timeout: self.frame(request_timeout=request_timeout),
            spec,
        )

    def get_by_text(
        self,
        query: str | re.Pattern[str],
        *,
        exact: bool = False,
        cross_soft_wraps: bool = False,
    ) -> Locator:
        return self.locator(TextSpec(query, exact, cross_soft_wraps))

    def row(self, index: int) -> Locator:
        return self.locator(row_spec(index))

    def region(self, value: Region) -> Locator:
        return self.locator(region_spec(value))

    def filter(self, style: StyleFilter) -> Locator:
        return self.locator(style_spec(style))

    def wait_for_text(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return self.wait_for(
            {"predicate": "contains", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for(
        self,
        predicate: JsonObject,
        *,
        stable_for: Duration | int | float = Duration(0),
        timeout: Duration | int | float = Duration.seconds(10),
        frame: FrameRequest = FrameRequest(),
    ) -> ScreenWaitResult:
        response = self._session.control(
            screen_wait_command(
                predicate,
                stable_for=stable_for,
                timeout=timeout,
                frame=frame,
            )
        )
        return normalize_screen_wait(response)

    def wait_for_absence(
        self, text: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return self.wait_for(
            {"predicate": "absent", "text": text},
            stable_for=stable_for,
            timeout=timeout,
        )

    def wait_for_regex(
        self, pattern: str, *, stable_for: float = 0.0, timeout: float = 10.0
    ) -> ScreenWaitResult:
        return self.wait_for(
            {"predicate": "matches_regex", "pattern": pattern},
            stable_for=stable_for,
            timeout=timeout,
        )

    def capture_stable(
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
                self.frame(selected.frame),
            )
        response = self._session.control(
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

    def wait_for_idle(
        self,
        *,
        stable_for: float = 0.1,
        timeout: float = 10.0,
        allow_incomplete: bool = False,
    ) -> StableCapture:
        return self.capture_stable(
            stable_for=stable_for, timeout=timeout, allow_incomplete=allow_incomplete
        )

    def screenshot(
        self,
        destination: str | Path,
        point: JsonObject | None = None,
        *,
        request_timeout: float | None = None,
    ) -> Artifact:
        return self._session.request(
            {
                "method": "screenshot",
                "point": point or {"selector": "latest_presented"},
                "destination": str(destination),
                "renderer": "semantic",
                "viewport": None,
                "max_bytes": None,
            },
            DriverArtifactData,
            request_timeout=request_timeout,
        )


class Keyboard:
    def __init__(self, session: Session) -> None:
        self._session = session

    def press(
        self,
        key: str,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return self._session.control(
            keyboard_press_command(key),
            request_id=request_id,
            if_state=if_state,
        )

    def type(
        self,
        text: str,
        *,
        caller_intent: str | None = None,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return self._session.control(
            keyboard_type_command(text, caller_intent=caller_intent),
            request_id=request_id,
            if_state=if_state,
        )

    def paste(
        self,
        data: str | bytes | memoryview,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
    ) -> ControlResult:
        return self._session.control(
            keyboard_paste_command(data),
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
            keyboard_sequence_command(
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
            pointer_click_command(column, row, button=button, strict=strict),
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
            pointer_scroll_command(column, row, lines, strict=strict),
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
            pointer_multi_click_command(column, row, count, button=button, strict=strict),
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
            pointer_drag_command(
                from_column,
                from_row,
                to_column,
                to_row,
                button=button,
                steps=steps,
                strict=strict,
            ),
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
            pointer_focus_command(focused, strict=strict),
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
        self,
        *,
        all_states: bool = False,
        controls: bool = False,
        offset: int = 0,
        limit: int = 256,
    ) -> TimelinePage:
        return self._session.request(
            {
                "method": "timeline",
                "all_states": all_states,
                "controls": controls,
                "offset": offset,
                "limit": limit,
            },
            DriverTimelineData,
        )

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
            events_page_request(
                from_event_seq=from_event_seq,
                limit=limit,
                metadata_only=metadata_only,
            ),
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
        return self._session.request(
            native_screenshot_request(destination, point=point, viewport=viewport),
            DriverArtifactData,
        )


class Evidence:
    def __init__(self, session: Session) -> None:
        self._session = session

    def completion(self) -> Completion:
        return self._session.completion()

    def complete(self, *, timeout: float = 10.0) -> Completion:
        return self._session.complete(
            timeout_ms=seconds_to_milliseconds(timeout),
            request_timeout=timeout + 1.0,
        )

    def verify(
        self,
        level: Literal["structure", "content", "durability_record"] = "content",
        *,
        request_timeout: float | None = None,
    ) -> VerificationReport:
        result = self._session.request(
            evidence_verify_request(level),
            DriverVerificationData,
            request_timeout=request_timeout,
        )
        return result.report

    def bundle(
        self,
        destination: str | Path,
        profile: Literal["screen", "debug", "forensic"] = "screen",
        *,
        request_timeout: float | None = None,
    ) -> BundleResult:
        return self._session.request(
            evidence_bundle_request(destination, profile),
            DriverBundleData,
            request_timeout=request_timeout,
        )


class Output:
    def __init__(self, session: Session) -> None:
        self._session = session

    def wait_for_bytes(self, data: bytes | memoryview, *, timeout: float = 10.0) -> WaitOutcome:
        return self._session.wait_for_output(
            output_wait_payload(data),
            timeout_ms=seconds_to_milliseconds(timeout),
        )

    def wait_for_text(self, text: str, *, timeout: float = 10.0) -> WaitOutcome:
        return self.wait_for_bytes(text.encode(), timeout=timeout)
