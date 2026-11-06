"""Asynchronous Termtrace wrappers. Generated from the native stub; run `just generate`."""

from __future__ import annotations

import asyncio
import builtins
import os
import typing
from collections.abc import Coroutine
from typing import TYPE_CHECKING, Any

from termtrace import _termtrace as _native
from termtrace import types
from termtrace._async import (
    LocatorBase,
    RecordingBase,
    Wrapper,
    launch,
    offload_mutation,
    run_cancellable,
)
from termtrace._termtrace import *
from termtrace._termtrace import CancelToken

if TYPE_CHECKING:
    from typing_extensions import Self

    from termtrace import editing

__all__ = [
    "AsyncActionBuilder",
    "AsyncDiagnostics",
    "AsyncEvents",
    "AsyncEvidence",
    "AsyncInstrumentation",
    "AsyncLocator",
    "AsyncNative",
    "AsyncOriginalRecording",
    "AsyncOutput",
    "AsyncPointer",
    "AsyncRecording",
    "AsyncRegistry",
    "AsyncScreen",
    "AsyncSession",
    "AsyncSystem",
    "AsyncTermtrace",
    "AsyncTmux",
]


class AsyncActionBuilder(Wrapper):
    r"""Asynchronous ActionBuilder: Fluent builder for atomic, state-guarded input sequences."""

    @property
    def request_id(self) -> builtins.str:
        r"""Identity of the next send; readable before any `run()` and stable once sealed."""
        return self._sync.request_id

    @property
    def sealed(self) -> builtins.bool:
        r"""Whether `run()` has frozen the request."""
        return self._sync.sealed

    def with_request_id(self, request_id: typing.Any) -> Self:
        r"""Replace the pre-allocated request identity before sealing.

        # Errors

        Raises `TypeError` for a value that is not `str` or `uuid.UUID`, and `TermtraceError`
        (`invalid_operation`) for an invalid UUID or a sealed builder.
        """
        self._sync.with_request_id(request_id)
        return self

    def if_state(self, state_hash: builtins.str) -> Self:
        r"""Enforce that the terminal must be at the specified state hash before executing.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.if_state(state_hash)
        return self

    def send_text(self, text: builtins.str) -> Self:
        r"""Append UTF-8 text to send to the terminal.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.send_text(text)
        return self

    def send_key(self, key: builtins.str) -> Self:
        r"""Append a key chord (e.g. 'Enter', 'ctrl+c', 'ctrl+shift+F5').

        # Errors

        Raises `ValueError` when `key` is not a valid key chord or names an unknown key, and
        `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.send_key(key)
        return self

    def send_bytes(self, bytes: typing.Sequence[builtins.int]) -> Self:
        r"""Append exact raw bytes (`bytes`, `bytearray`, or `memoryview`) to send to the terminal.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.send_bytes(bytes)
        return self

    def paste(self, text: builtins.str) -> Self:
        r"""Send bracketed paste text.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.paste(text)
        return self

    def pace(self, delay_ms: builtins.int) -> Self:
        r"""Set delay pacing between consecutive action atoms in milliseconds; paced text is typed
        one character at a time.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.pace(delay_ms)
        return self

    def intent(self, intent: builtins.str) -> Self:
        r"""Attach a human-readable caller intent retained with the input operations.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.intent(intent)
        return self

    def verbose(self, enabled: builtins.bool = True) -> Self:
        r"""Report per-wire-atom outcomes in `ActionResult.input["atoms"]`.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.verbose(enabled)
        return self

    def allow_incomplete(self, enabled: builtins.bool = True) -> Self:
        r"""Return an unmet postcondition as `matched == False` instead of raising
        `postcondition_incomplete`.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.allow_incomplete(enabled)
        return self

    def projection(
        self,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
    ) -> Self:
        r"""Request the terminating frame's detail and half-open region.

        # Errors

        Raises `ValueError` for an invalid detail or region, and `TermtraceError`
        (`invalid_operation`) once sealed.
        """
        self._sync.projection(detail, region)
        return self

    def until_screen(
        self, pattern: builtins.str, timeout_secs: builtins.float | None = None
    ) -> Self:
        r"""Wait until the screen contains the specified pattern after applying input.

        # Errors

        Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range, and
        `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.until_screen(pattern, timeout_secs)
        return self

    def until_screen_where(
        self,
        predicate: types.ScreenPredicateInput,
        timeout_ms: builtins.int | None = None,
    ) -> Self:
        r"""Wait after applying input until a presented-screen predicate holds.

        # Errors

        Raises `ValueError` for an invalid predicate or a timeout above the wait bound, and
        `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.until_screen_where(predicate, timeout_ms)
        return self

    def until_idle(
        self,
        stable_for_ms: builtins.int | None = None,
        timeout_secs: builtins.float | None = None,
        reaction_timeout_ms: builtins.int | None = None,
    ) -> Self:
        r"""Wait until the screen changes after applying input and then stays unchanged; the change
        must start within `reaction_timeout_ms`, or the condition settles as `unchanged`.

        # Errors

        Raises `ValueError` when a duration is negative, not finite, or out of range, and
        `TermtraceError` (`invalid_operation`) once sealed.
        """
        self._sync.until_idle(stable_for_ms, timeout_secs, reaction_timeout_ms)
        return self

    def seal(self) -> None:
        r"""Freeze the exact request without sending it; idempotent."""
        return self._sync.seal()

    def run(self) -> Coroutine[Any, Any, ActionResult]:
        r"""Seal the request and apply it, returning separate input and postcondition outcomes.

        Calling `run()` again resends the identical sealed request: the actor applies it at most
        once and returns the same `sequence_id`.

        # Errors

        Raises `TermtraceError` or a subclass with `request_id`, `effect`, and `retry`; an unmet
        postcondition raises `postcondition_incomplete` with the applied `result` unless
        `allow_incomplete()` was set.
        """
        self._sync.seal()
        return offload_mutation(self._pending, lambda: self._sync.run())

    async def execute(self) -> ResolvedState | None:
        r"""Legacy fresh request: send the current request with the current identity, then start over
        empty with a new identity; returns the terminating frame summary.

        # Errors

        Raises `TermtraceError` (`invalid_operation`) once sealed, and otherwise `TermtraceError` or
        a subclass when the session cannot be attached or the recorder rejects or cannot complete
        the queued actions.
        """
        return await offload_mutation(self._pending, lambda: self._sync.execute())


class AsyncDiagnostics(Wrapper):
    r"""Asynchronous Diagnostics: Replay analysis of one canonical session."""

    def __init__(
        self, path: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> None:
        self._sync = _native.Diagnostics(path)
        self._pending = set()

    async def timeline(
        self,
        offset: builtins.int = 0,
        limit: builtins.int = 100,
        include_hidden: builtins.bool = False,
        controls: builtins.bool = False,
        start_monotonic_ns: builtins.int | None = None,
        end_monotonic_ns: builtins.int | None = None,
    ) -> types.TimelinePage:
        r"""One bounded timeline page.
        # Errors
        Rejects a limit outside 1..=4096 or a partial or empty range.
        """
        return await asyncio.to_thread(
            lambda: self._sync.timeline(
                offset,
                limit,
                include_hidden,
                controls,
                start_monotonic_ns,
                end_monotonic_ns,
            )
        )

    async def diff(
        self, from_: types.StatePoint, to: types.StatePoint
    ) -> types.SemanticDiff:
        r"""Semantic difference between two states.
        # Errors
        Propagates endpoint validation and replay failures.
        """
        return await asyncio.to_thread(lambda: self._sync.diff(from_, to))

    async def explain(
        self, target: types.StatePoint | None = None, context: builtins.int = 8
    ) -> types.ExplainReport:
        r"""Source-bound VT explanation of one state.
        # Errors
        Propagates replay and validation failures.
        """
        return await asyncio.to_thread(lambda: self._sync.explain(target, context))

    async def diagnose(
        self,
        checks: typing.Sequence[types.DiagnosticCheck] | None = None,
        broad_change_percent: builtins.int = 50,
    ) -> types.DiagnoseReport:
        r"""Conservative diagnostics, including native evidence limitations.
        # Errors
        Rejects a change percent above 100 and propagates replay failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.diagnose(checks, broad_change_percent)
        )

    async def last_action(
        self, broad_change_percent: builtins.int = 50
    ) -> types.LastActionAnalysis:
        r"""Triage of the most recent action.
        # Errors
        Rejects a change percent above 100 and propagates absent-action and replay failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.last_action(broad_change_percent)
        )

    async def markers(
        self, fps: builtins.int = 30
    ) -> builtins.list[types.MarkerRecord]:
        r"""Marker positions and nominal frames.
        # Errors
        Rejects zero fps and propagates canonical evidence failures.
        """
        return await asyncio.to_thread(lambda: self._sync.markers(fps))

    async def cache_status(self) -> types.ReplayCacheStatus:
        r"""Replay cache inventory.
        # Errors
        Propagates filesystem failures.
        """
        return await asyncio.to_thread(lambda: self._sync.cache_status())

    async def cache_prune(self) -> types.ReplayCachePruneReport:
        r"""Prune disposable replay checkpoints.
        # Errors
        Propagates filesystem failures.
        """
        return await asyncio.to_thread(lambda: self._sync.cache_prune())

    async def review_summary(self) -> types.ReviewSummary:
        r"""Read-only forensic review summary.
        # Errors
        Propagates native-manifest and canonical validation failures.
        """
        return await asyncio.to_thread(lambda: self._sync.review_summary())

    async def review_search(self, query: builtins.str) -> types.ReviewSearchResult:
        r"""Semantic viewport search.
        # Errors
        Propagates replay and query failures.
        """
        return await asyncio.to_thread(lambda: self._sync.review_search(query))

    async def review_export(
        self,
        state_id: builtins.int,
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
    ) -> types.ReviewExportManifest:
        r"""Export one reviewed state without replacing an existing destination.
        # Errors
        Propagates replay and publication failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.review_export(state_id, destination)
        )


class AsyncEvents(Wrapper):
    r"""Asynchronous Events: Filtered canonical events retaining timestamps, hashes and exact byte summaries."""

    def __init__(
        self, path: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> None:
        self._sync = _native.Events(path)
        self._pending = set()

    async def page(
        self,
        cursor: types.EventPageCursor | None = None,
        limit: builtins.int = 100,
        metadata_only: builtins.bool = False,
        event_types: typing.Sequence[builtins.str] | None = None,
    ) -> types.ProjectedEventPage:
        r"""Read checked event projections; empty filters select every canonical event kind.

        # Errors
        Rejects invalid filters, limits, or cursors and invalid or replaced canonical evidence.
        """
        return await asyncio.to_thread(
            lambda: self._sync.page(cursor, limit, metadata_only, event_types)
        )

    async def follow(
        self, next_event_seq: builtins.int = 0
    ) -> types.EventFollowPosition:
        r"""Ask the original actor for its durable follow positions, or validate completion.

        # Errors
        Propagates actor, transport, and canonical completion validation failures.
        """
        return await asyncio.to_thread(lambda: self._sync.follow(next_event_seq))

    def cursor_to_json(self, cursor: types.EventPageCursor) -> builtins.str:
        r"""Canonical JSON text of a cursor, for storing and resuming later.

        # Errors
        Rejects malformed cursors.
        """
        return self._sync.cursor_to_json(cursor)

    def cursor_from_json(self, text: builtins.str) -> types.EventPageCursor:
        r"""Decode a cursor from its canonical JSON text.

        # Errors
        Rejects malformed JSON or cursor shapes.
        """
        return self._sync.cursor_from_json(text)


class AsyncEvidence(Wrapper):
    r"""Asynchronous Evidence: Evidence artifacts, verification, signing, and recovery for one canonical session."""

    def __init__(
        self, path: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> None:
        self._sync = _native.Evidence(path)
        self._pending = set()

    async def save(
        self,
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        formats: typing.Sequence[types.SaveFormat],
        point: types.StatePoint | None = None,
        include_hidden: builtins.bool = False,
    ) -> types.SaveOutcome:
        r"""Atomically save the requested representations of one state.
        # Errors
        Propagates state resolution, rendering, and publication failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.save(destination, formats, point, include_hidden)
        )

    async def bundle(
        self,
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        profile: types.BundleProfile,
    ) -> types.BundleManifest:
        r"""Create an evidence bundle with an explicit profile.
        # Errors
        Propagates bundle failures.
        """
        return await asyncio.to_thread(lambda: self._sync.bundle(destination, profile))

    async def verify_bundle(
        self,
        directory: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        profile: types.BundleProfile,
    ) -> types.BundleManifest:
        r"""Verify a bundle against this session and an explicit profile.
        # Errors
        Propagates invalid, tampered, wrong-source, and wrong-profile failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.verify_bundle(directory, profile)
        )

    async def verify(
        self,
        level: types.VerificationLevel | None = None,
        allowed_signers: str
        | bytes
        | os.PathLike[str]
        | os.PathLike[bytes]
        | None = None,
    ) -> types.VerificationReport:
        r"""Verify the session; validity, authenticity, and trust stay separate facts."""
        return await asyncio.to_thread(
            lambda: self._sync.verify(level, allowed_signers)
        )

    async def sign(
        self,
        key: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        signer: builtins.str,
    ) -> types.ExactPath:
        r"""Sign an exact receipt; signing never implies trust.
        # Errors
        Propagates verification, key, signing, and publication failures.
        """
        return await asyncio.to_thread(lambda: self._sync.sign(key, signer))

    async def recover(
        self, destination: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> types.RecoveryReport:
        r"""Recover the valid prefix into a new session directory without changing the source.
        # Errors
        Propagates invalid prefix and destination failures.
        """
        return await asyncio.to_thread(lambda: self._sync.recover(destination))

    async def recover_log(
        self, destination: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> types.RecoveryReport:
        r"""Recover the valid prefix of this event log into a new log.
        # Errors
        Propagates invalid prefix and destination failures.
        """
        return await asyncio.to_thread(lambda: self._sync.recover_log(destination))

    async def asciicast(
        self, destination: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> types.ExactPath:
        r"""Export an asciicast and return its path.
        # Errors
        Propagates replay and publication failures.
        """
        return await asyncio.to_thread(lambda: self._sync.asciicast(destination))


class AsyncInstrumentation(Wrapper):
    r"""Asynchronous Instrumentation: Application instrumentation; spans end only when the caller ends them."""

    def __init__(
        self, source: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> None:
        self._sync = _native.Instrumentation(source)
        self._pending = set()

    @property
    def process_instance_id(self) -> builtins.str:
        r"""Process identity of every event and span of this emitter."""
        return self._sync.process_instance_id

    async def event(
        self,
        name: builtins.str,
        attributes: typing.Mapping[builtins.str, types.AnyValue] | None = None,
    ) -> types.ApplicationAcknowledgement:
        r"""Emit one event and return the actor acknowledgement.
        # Errors
        Rejects unsupported attributes and propagates control failures, without retries.
        """
        return await asyncio.to_thread(lambda: self._sync.event(name, attributes))

    async def start_span(
        self,
        name: builtins.str,
        parent_span_id: builtins.str | None = None,
        attributes: typing.Mapping[builtins.str, types.AnyValue] | None = None,
    ) -> types.ApplicationAcknowledgement:
        r"""Start a span; dropping this object never ends it.
        # Errors
        Propagates validation and control failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.start_span(name, parent_span_id, attributes)
        )

    async def end_span(
        self,
        name: builtins.str,
        span_id: builtins.str,
        attributes: typing.Mapping[builtins.str, types.AnyValue] | None = None,
    ) -> types.ApplicationAcknowledgement:
        r"""End one span.
        # Errors
        Propagates validation and control failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.end_span(name, span_id, attributes)
        )


class AsyncLocator(LocatorBase):
    r"""Asynchronous Locator: Lazy locator bound to a session; every operation captures one fresh frame."""

    @property
    def spec(self) -> LocatorSpec:
        r"""The bound specification."""
        return self._sync.spec

    def and_(self, other: LocatorSpec) -> Self:
        r"""Combine with another specification without capturing.

        # Errors

        Raises `QueryError` when the scopes conflict or both hold a text matcher or style filter.
        """
        self._sync.and_(other)
        return self

    def in_row(self, row: builtins.int) -> Self:
        r"""Restrict to one absolute row without capturing."""
        self._sync.in_row(row)
        return self

    def in_region(self, region: types.ScreenRegion) -> Self:
        r"""Restrict to one nonempty absolute half-open region without capturing.

        # Errors

        Raises `ValueError` when `region` is malformed and `QueryError` when it is empty.
        """
        self._sync.in_region(region)
        return self

    def with_style(self, style: types.StyleFilterInput) -> Self:
        r"""Require a style filter without capturing.

        # Errors

        Raises `QueryError` when `style` is malformed.
        """
        self._sync.with_style(style)
        return self

    def across_soft_wraps(self, enabled: builtins.bool = True) -> Self:
        r"""Let text matches continue across native soft-wrapped rows."""
        self._sync.across_soft_wraps(enabled)
        return self

    async def evaluate(self) -> QueryResult:
        r"""Capture one frame and evaluate it once.

        # Errors

        Raises `QueryError` when the specification does not fit the frame and `TermtraceError`
        or a subclass when the capture fails.
        """
        return await asyncio.to_thread(lambda: self._sync.evaluate())

    async def all(self) -> builtins.list[ScreenMatch]:
        r"""Capture once and return every match.

        # Errors

        Raises the errors of `evaluate`.
        """
        return await asyncio.to_thread(lambda: self._sync.all())

    async def count(self) -> builtins.int:
        r"""Capture once and count matches.

        # Errors

        Raises the errors of `evaluate`.
        """
        return await asyncio.to_thread(lambda: self._sync.count())

    async def single(self) -> ScreenMatch:
        r"""Capture once and require exactly one match.

        # Errors

        Raises the errors of `evaluate`, and `QueryError` with kind `no_match` or
        `multiple_matches`.
        """
        return await asyncio.to_thread(lambda: self._sync.single())

    async def first(self) -> ScreenMatch:
        r"""Capture once and select the first match.

        # Errors

        Raises the errors of `evaluate`, and `QueryError` with kind `no_match`.
        """
        return await asyncio.to_thread(lambda: self._sync.first())

    async def nth(self, index: builtins.int) -> ScreenMatch:
        r"""Capture once and select one zero-based match.

        # Errors

        Raises the errors of `evaluate`, and `QueryError` with kind `index_out_of_bounds`.
        """
        return await asyncio.to_thread(lambda: self._sync.nth(index))

    async def wait_for(
        self,
        condition: types.LocatorCondition | None = None,
        timeout_secs: builtins.float | None = None,
        interval_secs: builtins.float | None = None,
        stable_for_secs: builtins.float | None = None,
        cancel: CancelToken | None = None,
    ) -> LocatorWaitResult:
        r"""Poll until `condition` (`"exists"`, `"absent"`, or `{"count": n}`) holds in one
        unchanged state for `stable_for_secs`.

        Another thread may call `cancel.cancel()`. A responsive session settles within about one
        250 ms capture quantum plus a round trip; a session busy with other work can delay the
        return by up to about 5.25 s. No input is ever sent.

        # Errors

        Raises `ValueError` for malformed durations, `QueryError` for an invalid condition, zero
        timeout or interval, or `stable_for_secs` above `timeout_secs`, `TimeoutError` (kind
        `locator_timeout`, with the last `frame`) when unsatisfied in time, `CancelledError`
        (kind `cancelled`, with the last `frame`) after cancellation, and `TermtraceError` or a
        subclass when a capture fails.
        """

        def call(token: CancelToken) -> typing.Any:
            return self._sync.wait_for(
                condition, timeout_secs, interval_secs, stable_for_secs, token
            )

        return await run_cancellable(call, cancel)


class AsyncNative(Wrapper):
    r"""Asynchronous Native: Native replay operations for one canonical session."""

    async def screenshot(
        self,
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        point: types.StatePoint | None = None,
        timeout_secs: builtins.float | None = None,
    ) -> types.CaptureOutcome:
        r"""Replay this completed session in Ghostty and capture `point` (default: the latest
        presented state) as a PNG in the new `destination` directory, with its manifest and
        replay endpoint. `timeout_secs` bounds the replay terminal's startup.

        # Errors

        Raises `TermtraceError` or a subclass when the `native-screenshot` capability is blocked,
        the session is live, `point` does not resolve, `destination` exists, or capture fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.screenshot(destination, point, timeout_secs)
        )


class AsyncOriginalRecording(Wrapper):
    r"""Asynchronous OriginalRecording: An original recording: a canonical session whose terminal pixels are captured while it runs."""

    @property
    def session_id(self) -> builtins.str:
        r"""Unique session identifier."""
        return self._sync.session_id

    @property
    def path(self) -> builtins.str:
        r"""Filesystem path to the canonical session directory."""
        return self._sync.path

    @property
    def path_bytes(self) -> bytes:
        r"""Exact canonical session directory bytes."""
        return self._sync.path_bytes

    async def is_active(self) -> builtins.bool:
        r"""Whether this handle still owns the recording."""
        return await asyncio.to_thread(lambda: self._sync.is_active())

    def session(self) -> AsyncSession:
        r"""Live control and observation of the canonical session; resizes move the captured window."""
        return AsyncSession._wrap(self._sync.session())

    async def capture_status(self) -> types.OriginalCaptureStatus:
        r"""Supervisor status, with `abandoned` derived when the supervisor is gone.

        # Errors

        Raises `TermtraceError` when the status sidecar is missing or invalid.
        """
        return await asyncio.to_thread(lambda: self._sync.capture_status())

    async def finish(
        self,
        timeout_ms: builtins.int | None = None,
        request_id: typing.Any | None = None,
    ) -> types.OriginalRecording:
        r"""Stop the application and wait until the capture is published.

        Returns `completion`, `canonical_success` and `capture`, the published media artifact.

        # Errors

        Raises `OriginalCaptureFailedError` when the capture did not publish,
        `OriginalFinalizationTimeoutError` when it was still finalizing at the deadline (the
        recording stays owned), and stop failures with `request_id`.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.finish(timeout_ms, request_id)
        )

    async def wait_finished(
        self, timeout_ms: builtins.int | None = None
    ) -> types.OriginalRecording:
        r"""Wait without stopping until the application exits and the capture is published.

        # Errors

        As `finish`, without a stop request.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.wait_finished(timeout_ms)
        )

    async def force_close(self) -> None:
        r"""Kill the application's process group and the capture supervisor; the capture is
        abandoned. A no-op once the recording finished or was detached.

        # Errors

        Raises `TermtraceError` when the processes cannot be terminated and reaped, leaving
        the recording owned.
        """
        return await offload_mutation(self._pending, lambda: self._sync.force_close())

    def detach(self) -> None:
        r"""Stop managing the recording without stopping it.

        # Errors

        Raises `RuntimeError` when the recording already finished or was detached.
        """
        return self._sync.detach()


class AsyncOutput(Wrapper):
    r"""Asynchronous Output: Source-bound canonical PTY-byte pages."""

    def __init__(
        self, path: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> None:
        self._sync = _native.Output(path)
        self._pending = set()

    async def page(
        self, cursor: types.OutputPageCursor | None = None, limit: builtins.int = 65536
    ) -> types.ByteOutputPage:
        r"""Read one checked finite byte snapshot.

        # Errors
        Rejects foreign or modified cursors, invalid limits, invalid complete evidence, and
        replacement.
        """
        return await asyncio.to_thread(lambda: self._sync.page(cursor, limit))

    async def wait_for(
        self,
        pattern: typing.Sequence[builtins.int],
        timeout_secs: builtins.float | None = None,
    ) -> types.WaitOutcome:
        r"""Wait for exact bytes and return the shared structured outcome.

        # Errors
        Rejects empty bytes, invalid timeouts, invalid evidence, or completion without a match.
        """
        return await asyncio.to_thread(
            lambda: self._sync.wait_for(pattern, timeout_secs)
        )

    def cursor_to_json(self, cursor: types.OutputPageCursor) -> builtins.str:
        r"""Canonical JSON text of a cursor, for storing and resuming later.

        # Errors
        Rejects malformed cursors.
        """
        return self._sync.cursor_to_json(cursor)

    def cursor_from_json(self, text: builtins.str) -> types.OutputPageCursor:
        r"""Decode a cursor from its canonical JSON text.

        # Errors
        Rejects malformed JSON or cursor shapes.
        """
        return self._sync.cursor_from_json(text)


class AsyncPointer(Wrapper):
    r"""Asynchronous Pointer: Mode-aware pointer, focus, and text-activation operations with typed results."""

    async def click(
        self,
        column: builtins.int,
        row: builtins.int,
        button: builtins.str | None = None,
        *,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Press and release one button at a zero-based cell.

        # Errors

        Raises `ValueError` for an unknown button and `TermtraceError` or a subclass, with
        `request_id`, when the recorder rejects the click (with `strict`, when the application does
        not report mouse input).
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.click(
                column,
                row,
                button,
                strict=strict,
                request_id=request_id,
                if_state=if_state,
            ),
        )

    async def multi_click(
        self,
        column: builtins.int,
        row: builtins.int,
        count: builtins.int,
        button: builtins.str | None = None,
        *,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Emit `count` complete clicks at a zero-based cell.

        # Errors

        Raises as `click`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.multi_click(
                column,
                row,
                count,
                button,
                strict=strict,
                request_id=request_id,
                if_state=if_state,
            ),
        )

    async def drag(
        self,
        column: builtins.int,
        row: builtins.int,
        to_column: builtins.int,
        to_row: builtins.int,
        button: builtins.str | None = None,
        *,
        steps: builtins.int = 1,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Press at a zero-based cell, emit `steps` motion samples, and release at the end cell.

        # Errors

        Raises as `click`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.drag(
                column,
                row,
                to_column,
                to_row,
                button,
                steps=steps,
                strict=strict,
                request_id=request_id,
                if_state=if_state,
            ),
        )

    async def scroll(
        self,
        column: builtins.int,
        row: builtins.int,
        lines: builtins.int = 1,
        *,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Emit signed vertical wheel ticks at a zero-based cell; negative scrolls up.

        # Errors

        Raises as `click`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.scroll(
                column,
                row,
                lines,
                strict=strict,
                request_id=request_id,
                if_state=if_state,
            ),
        )

    async def focus(
        self,
        focused: builtins.bool,
        *,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Report a focus transition when the application enabled focus reporting.

        # Errors

        Raises as `click`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.focus(
                focused, strict=strict, request_id=request_id, if_state=if_state
            ),
        )

    async def activate(
        self,
        text: builtins.str,
        *,
        unique: builtins.bool = False,
        strict: builtins.bool = False,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.PointerOutcome:
        r"""Resolve visible row-contained text and click its center from one actor-ordered state.

        # Errors

        Raises as `click`; the codes `activation_text_not_found`, `activation_text_concealed`, and
        `activation_text_ambiguous` report resolution failures.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.activate(
                text,
                unique=unique,
                strict=strict,
                request_id=request_id,
                if_state=if_state,
            ),
        )


class AsyncRecording(RecordingBase):
    r"""Asynchronous Recording: An active recording session owning child processes and evidence buffers."""

    @property
    def session_id(self) -> builtins.str:
        r"""Unique session identifier."""
        return self._sync.session_id

    @property
    def path(self) -> builtins.str:
        r"""Filesystem path to the canonical session directory."""
        return self._sync.path

    async def is_active(self) -> builtins.bool:
        r"""Check if the recording session is still active."""
        return await asyncio.to_thread(lambda: self._sync.is_active())

    def instrumentation(self) -> AsyncInstrumentation:
        r"""Explicit application instrumentation; retain the emitter to share a process instance."""
        return AsyncInstrumentation._wrap(self._sync.instrumentation())

    def analysis(self) -> AsyncDiagnostics:
        r"""Typed analysis operations through the shared service."""
        return AsyncDiagnostics._wrap(self._sync.analysis())

    def evidence(self) -> AsyncEvidence:
        r"""Typed evidence operations through the shared service."""
        return AsyncEvidence._wrap(self._sync.evidence())

    def native(self) -> AsyncNative:
        r"""Native replay operations through the shared service."""
        return AsyncNative._wrap(self._sync.native())

    async def integrity(self) -> AsyncEvidence:
        r"""Typed integrity operations through the shared service."""
        return AsyncEvidence._wrap(
            await asyncio.to_thread(lambda: self._sync.integrity())
        )

    def output(self) -> AsyncOutput:
        r"""Source-bound exact PTY output pages."""
        return AsyncOutput._wrap(self._sync.output())

    def events(self) -> AsyncEvents:
        r"""Source-bound filtered canonical event projections."""
        return AsyncEvents._wrap(self._sync.events())

    def screen(self) -> AsyncScreen:
        r"""Access screen inspection capabilities.

        # Errors
        Raises `RuntimeError` when the recording has already been stopped or detached.
        """
        return AsyncScreen._wrap(self._sync.screen())

    def actions(self) -> AsyncActionBuilder:
        r"""Create an action builder for input automation.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached.
        """
        return AsyncActionBuilder._wrap(self._sync.actions(), self._pending)

    async def session(self) -> AsyncSession:
        r"""Get a logical session reference.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached.
        """
        return AsyncSession._wrap(await asyncio.to_thread(lambda: self._sync.session()))

    async def pointer_click(
        self,
        column: builtins.int,
        row: builtins.int,
        button: builtins.str | None = None,
        strict: builtins.bool | None = None,
    ) -> None:
        r"""Click mouse button at a terminal cell.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::pointer_click`.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.pointer_click(column, row, button, strict)
        )

    async def pointer_scroll(
        self,
        column: builtins.int,
        row: builtins.int,
        lines: builtins.int | None = None,
        strict: builtins.bool | None = None,
    ) -> None:
        r"""Scroll mouse wheel at a terminal cell.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::pointer_scroll`.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.pointer_scroll(column, row, lines, strict)
        )

    async def pointer_focus(
        self, focused: builtins.bool, strict: builtins.bool | None = None
    ) -> None:
        r"""Report terminal focus state.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::pointer_focus`.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.pointer_focus(focused, strict)
        )

    def pointer(self) -> AsyncPointer:
        r"""Mode-aware pointer, focus, and text-activation operations with typed results.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached.
        """
        return AsyncPointer._wrap(self._sync.pointer(), self._pending)

    async def resize(
        self,
        cols: builtins.int,
        rows: builtins.int,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.ControlAcknowledgement:
        r"""Resize the child PTY to `cols` by `rows` cells.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::resize`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.resize(
                cols, rows, request_id=request_id, if_state=if_state
            ),
        )

    async def signal(
        self,
        signal: types.ProcessSignal,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.ControlAcknowledgement:
        r"""Send one allow-listed signal to the child process group.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::signal`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.signal(signal, request_id=request_id, if_state=if_state),
        )

    async def mark(
        self,
        label: builtins.str,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.MarkOutcome:
        r"""Append a named marker to the canonical event stream.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::mark`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.mark(label, request_id=request_id, if_state=if_state),
        )

    async def send(
        self,
        steps: typing.Sequence[types.SendStepSpecInput],
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
        intent: builtins.str | None = None,
        pace_ms: builtins.int | None = None,
        verbose: builtins.bool = False,
        until: types.ConditionSpecInput | None = None,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
        report_changes: builtins.bool = False,
    ) -> typing.Any:
        r"""Apply ordered steps as `PySession::send`.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::send`.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.send(
                steps,
                request_id=request_id,
                if_state=if_state,
                intent=intent,
                pace_ms=pace_ms,
                verbose=verbose,
                until=until,
                detail=detail,
                region=region,
                report_changes=report_changes,
            ),
        )

    async def save(
        self,
        destination: builtins.str,
        formats: typing.Sequence[builtins.str] | None = None,
        state_hash: builtins.str | None = None,
        include_hidden: builtins.bool | None = None,
    ) -> types.SaveOutcome:
        r"""Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::save`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.save(destination, formats, state_hash, include_hidden)
        )

    async def read_output(
        self, offset: builtins.int | None = None, limit: builtins.int | None = None
    ) -> tuple[builtins.list[builtins.int], builtins.int, builtins.bool]:
        r"""Read raw child PTY output bytes using an optional byte offset cursor.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::read_output`.
        """
        return await asyncio.to_thread(lambda: self._sync.read_output(offset, limit))

    async def wait_for_output(
        self, pattern: builtins.str, timeout_secs: builtins.float | None = None
    ) -> builtins.bool:
        r"""Wait for exact child output bytes to appear in the canonical stream.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::wait_for_output`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.wait_for_output(pattern, timeout_secs)
        )

    async def read_events(
        self, cursor: builtins.int | None = None, limit: builtins.int | None = None
    ) -> builtins.list[types.Event]:
        r"""Read canonical event records as pretty JSON starting at an optional event sequence.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::read_events`.
        """
        return await asyncio.to_thread(lambda: self._sync.read_events(cursor, limit))

    async def explain(
        self,
        target_hash: builtins.str | None = None,
        context: builtins.int | None = None,
    ) -> types.ExplainReport:
        r"""Explain VT and semantic evidence around a target state point.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::explain`.
        """
        return await asyncio.to_thread(lambda: self._sync.explain(target_hash, context))

    async def diagnose(
        self, broad_change_percent: builtins.int | None = None
    ) -> types.DiagnoseReport:
        r"""Run conservative evidence-backed rendering diagnostics on the session.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::diagnose`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.diagnose(broad_change_percent)
        )

    async def last_action(
        self, broad_change_percent: builtins.int | None = None
    ) -> types.LastActionAnalysis:
        r"""Perform full triage analysis on the most recent canonical action.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
        as `PySession::last_action`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.last_action(broad_change_percent)
        )

    async def request_stop(
        self, request_id: typing.Any | None = None, wait_ms: builtins.int | None = None
    ) -> types.StopReport:
        r"""Ask the recording to end; with `wait_ms`, wait that long for validated completion.

        Returns `{"kind": "requested", ...}` or `{"kind": "completed", ...}`. A failed request
        leaves the recording attached; pass `request_id` to retry it with the same identity.

        # Errors

        Raises `RuntimeError` when already stopped or detached, and `TermtraceError` or a subclass
        with `request_id` when the stop is rejected or completion is not published in time.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.request_stop(request_id, wait_ms)
        )

    async def wait_for_completion(
        self, timeout_ms: builtins.int | None = None
    ) -> types.SessionCompletion:
        r"""Wait for validated completion evidence and reap the recorder; returns the completion.

        # Errors

        Raises `RuntimeError` when already stopped or detached, and `TermtraceError` or a subclass
        when completion is not published in time, leaving the recording attached.
        """
        return await offload_mutation(
            self._pending, lambda: self._sync.wait_for_completion(timeout_ms)
        )

    async def status(self) -> types.SessionStatus:
        r"""Live status `{"kind": "live", ...}` or completion `{"kind": "completed", ...}`, also after
        stop.

        # Errors

        Raises `TermtraceError` or a subclass when the session cannot be attached or read.
        """
        return await asyncio.to_thread(lambda: self._sync.status())

    async def discard_completed(self) -> None:
        r"""Delete this completed canonical session, reaping an owned recorder.

        # Errors

        Raises `TermtraceError` or a subclass when completion is missing or invalid or the
        session cannot be removed durably; an owned recording stays attached.
        """
        return await asyncio.to_thread(lambda: self._sync.discard_completed())

    async def force_close(self) -> None:
        r"""Forcefully terminate and reap the recorded process group and owned recorder.

        # Errors

        Raises `TermtraceError` or a subclass when the process group or recorder cannot be
        terminated and reaped, leaving the recording attached.
        """
        return await asyncio.to_thread(lambda: self._sync.force_close())

    async def detach(self) -> None:
        r"""Detach the recording so it continues running in the background.

        # Errors

        Raises `RuntimeError` when the recording has already been stopped or detached.
        """
        return await asyncio.to_thread(lambda: self._sync.detach())


class AsyncRegistry(Wrapper):
    r"""Asynchronous Registry: Session registry: aliases, lifecycle, retention, and restart."""

    def __init__(
        self,
        root: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        binary: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
    ) -> None:
        self._sync = _native.Registry(root, binary)
        self._pending = set()

    async def restart(
        self,
        alias: builtins.str,
        options: types.RestartConfigurationInput | None = None,
        environment: types.EnvironmentSpecInput | None = None,
        timeout_ms: builtins.float | None = None,
    ) -> types.RestartReport:
        r"""Restart through shared readiness, lineage, and alias compare-and-swap.
        # Errors
        Propagates option, predecessor, successor, orphan, and retarget failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.restart(alias, options, environment, timeout_ms)
        )

    async def list(
        self, include_all: builtins.bool = False
    ) -> builtins.list[types.RegistryListEntry]:
        r"""Registered sessions; unhealthy rows keep their diagnostics.
        # Errors
        Propagates state-root errors.
        """
        return await asyncio.to_thread(lambda: self._sync.list(include_all))

    async def register(
        self,
        alias: builtins.str,
        session: str | bytes | os.PathLike[str] | os.PathLike[bytes],
    ) -> types.RegistryEntry:
        r"""Register a session under an alias.
        # Errors
        Propagates invalid aliases, collisions, security, and identity errors.
        """
        return await asyncio.to_thread(lambda: self._sync.register(alias, session))

    async def resolve(self, alias: builtins.str) -> types.RegistryEntry | None:
        r"""Resolve an alias; `None` only when no alias record exists.
        # Errors
        Corruption, permissions, missing evidence, and invalid aliases propagate.
        """
        return await asyncio.to_thread(lambda: self._sync.resolve(alias))

    async def lifecycle(self, alias: builtins.str) -> types.RegistrySessionResolution:
        r"""Derived lifecycle of an alias, including pending and incompatible states.
        # Errors
        Registry-record failures propagate.
        """
        return await asyncio.to_thread(lambda: self._sync.lifecycle(alias))

    async def remove(self, alias: builtins.str) -> types.RegistryEntry:
        r"""Remove an alias; canonical evidence is kept.
        # Errors
        Missing aliases, permissions, and invalid records propagate.
        """
        return await asyncio.to_thread(lambda: self._sync.remove(alias))

    async def gc(
        self,
        older_than_days: builtins.int = 30,
        keep_latest: builtins.int = 20,
        max_completed_bytes: builtins.int | None = None,
        apply: builtins.bool = False,
    ) -> types.GarbageCollectionReport:
        r"""Retention planning, applied only when `apply` is true.
        # Errors
        Inspection and deletion failures propagate.
        """
        return await asyncio.to_thread(
            lambda: self._sync.gc(
                older_than_days, keep_latest, max_completed_bytes, apply
            )
        )


class AsyncScreen(Wrapper):
    r"""Asynchronous Screen: Screen inspection capability attached to a live or completed session."""

    async def frame(
        self,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
    ) -> ScreenFrame:
        r"""Return the latest presented screen frame with the requested detail (`none`, `text`,
        `spans` by default, or `cells`, which requires `region`) and optional half-open region.

        # Errors

        Raises `ValueError` when `detail` or `region` is malformed, and `TermtraceError` or a
        subclass when the session cannot be attached or the frame cannot be projected.
        """
        return await asyncio.to_thread(lambda: self._sync.frame(detail, region))

    async def frame_at(
        self,
        point: types.StatePoint,
        include_hidden: builtins.bool = False,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
    ) -> ScreenFrame:
        r"""Return a frame at any state point of a completed session, or the latest presented
        state of a live one.

        # Errors

        Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
        a live session is asked for another point or hidden states, or resolution fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.frame_at(point, include_hidden, detail, region)
        )

    async def observe(
        self,
        point: types.StatePoint | None = None,
        include_hidden: builtins.bool = False,
    ) -> types.Observation:
        r"""Return the complete observation of a state point (latest presented by default).

        # Errors

        Raises `ValueError` when `point` is malformed, and `TermtraceError` or a subclass when a
        live session is asked for another point, or observation fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.observe(point, include_hidden)
        )

    async def capture_stable(
        self,
        stable_for_secs: builtins.float | None = None,
        timeout_secs: builtins.float | None = None,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
        allow_incomplete: builtins.bool = False,
        reaction: typing.Any | None = None,
    ) -> types.StableScreenCapture:
        r"""Capture one exact frame once it stays unchanged for `stable_for_secs`, optionally after a
        `reaction` (`{"changed_from": str | None, "timeout_secs": float}`).

        # Errors

        Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
        the capture fails or, unless `allow_incomplete`, does not settle.
        """
        return await asyncio.to_thread(
            lambda: self._sync.capture_stable(
                stable_for_secs,
                timeout_secs,
                detail,
                region,
                allow_incomplete,
                reaction,
            )
        )

    async def wait_for(
        self,
        predicate: types.ScreenPredicateInput,
        stable_for_secs: builtins.float | None = None,
        timeout_secs: builtins.float | None = None,
    ) -> types.ScreenWaitResult:
        r"""Wait on the actor for a screen predicate and return the outcome without raising when it
        is unmatched; `quality` tells a deadline from child completion or output closure.

        # Errors

        Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
        the actor rejects the predicate or the wait fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.wait_for(predicate, stable_for_secs, timeout_secs)
        )

    def locator(self, spec: LocatorSpec | None = None) -> AsyncLocator:
        r"""Bind a locator specification to this screen without capturing; the default matches
        every cell.
        """
        return AsyncLocator._wrap(self._sync.locator(spec))

    def get_by_text(
        self, text: builtins.str, exact: builtins.bool = False
    ) -> AsyncLocator:
        r"""Bind a nonempty literal text locator; `exact` compares the complete row or region text.

        # Errors

        Raises `QueryError` when `text` is empty.
        """
        return AsyncLocator._wrap(self._sync.get_by_text(text, exact))

    def get_by_regex(self, pattern: typing.Any) -> AsyncLocator:
        r"""Bind a Rust regex syntax locator.

        # Errors

        Raises `TypeError` for anything but a `str` (including `re.Pattern`) and `QueryError`
        when the pattern is invalid or can match zero-width text.
        """
        return AsyncLocator._wrap(self._sync.get_by_regex(pattern))

    def row(self, row: builtins.int) -> AsyncLocator:
        r"""Bind one absolute row."""
        return AsyncLocator._wrap(self._sync.row(row))

    def region(self, region: types.ScreenRegion) -> AsyncLocator:
        r"""Bind one nonempty absolute half-open region.

        # Errors

        Raises `ValueError` when `region` is malformed and `QueryError` when it is empty.
        """
        return AsyncLocator._wrap(self._sync.region(region))

    def filter(self, style: types.StyleFilterInput) -> AsyncLocator:
        r"""Bind a cell appearance filter.

        # Errors

        Raises `QueryError` when `style` is malformed.
        """
        return AsyncLocator._wrap(self._sync.filter(style))

    async def text(self) -> builtins.str:
        r"""Return the normalized plain text of the latest presented frame.

        # Errors

        Raises `TermtraceError` or a subclass when the session cannot be attached or the frame
        cannot be captured.
        """
        return await asyncio.to_thread(lambda: self._sync.text())

    async def wait_for_text(
        self, pattern: builtins.str, timeout_secs: builtins.float | None = None
    ) -> ResolvedState:
        r"""Wait until the presented screen contains the specified text pattern.

        # Errors

        Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
        `TimeoutError` when the text is not presented in time, and `TermtraceError` or a subclass
        when the session cannot be attached or the wait fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.wait_for_text(pattern, timeout_secs)
        )

    async def wait_for_idle(
        self, timeout_secs: builtins.float | None = None
    ) -> ResolvedState:
        r"""Wait until the presented screen remains unchanged for an idle window.

        # Errors

        Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
        `TimeoutError` when the screen does not settle in time, and `TermtraceError` or a subclass
        when the session cannot be attached or the capture fails.
        """
        return await asyncio.to_thread(lambda: self._sync.wait_for_idle(timeout_secs))


class AsyncSession(Wrapper):
    r"""Asynchronous Session: A logical Termtrace session reference."""

    @property
    def session_id(self) -> builtins.str:
        r"""Canonical session identity string."""
        return self._sync.session_id

    @property
    def path(self) -> builtins.str:
        r"""Filesystem path to the session directory, for display when it is not UTF-8."""
        return self._sync.path

    @property
    def path_bytes(self) -> bytes:
        r"""Exact session directory bytes."""
        return self._sync.path_bytes

    def instrumentation(self) -> AsyncInstrumentation:
        r"""Explicit application instrumentation; retain the emitter to share a process instance."""
        return AsyncInstrumentation._wrap(self._sync.instrumentation())

    def analysis(self) -> AsyncDiagnostics:
        r"""Typed analysis operations through the shared service."""
        return AsyncDiagnostics._wrap(self._sync.analysis())

    def evidence(self) -> AsyncEvidence:
        r"""Typed evidence operations through the shared service."""
        return AsyncEvidence._wrap(self._sync.evidence())

    def native(self) -> AsyncNative:
        r"""Native replay operations through the shared service."""
        return AsyncNative._wrap(self._sync.native())

    async def integrity(self) -> AsyncEvidence:
        r"""Typed integrity operations through the shared service."""
        return AsyncEvidence._wrap(
            await asyncio.to_thread(lambda: self._sync.integrity())
        )

    def output(self) -> AsyncOutput:
        r"""Source-bound exact PTY output pages."""
        return AsyncOutput._wrap(self._sync.output())

    def events(self) -> AsyncEvents:
        r"""Source-bound filtered canonical event projections."""
        return AsyncEvents._wrap(self._sync.events())

    def screen(self) -> AsyncScreen:
        r"""Access screen inspection capabilities."""
        return AsyncScreen._wrap(self._sync.screen())

    def actions(self) -> AsyncActionBuilder:
        r"""Create an action builder for input automation."""
        return AsyncActionBuilder._wrap(self._sync.actions(), self._pending)

    def pointer(self) -> AsyncPointer:
        r"""Mode-aware pointer, focus, and text-activation operations with typed results."""
        return AsyncPointer._wrap(self._sync.pointer(), self._pending)

    async def pointer_click(
        self,
        column: builtins.int,
        row: builtins.int,
        button: builtins.str | None = None,
        strict: builtins.bool | None = None,
    ) -> None:
        r"""Legacy click adapter over `pointer().click`; returns `None`.

        # Errors

        Raises `ValueError` when `button` is unknown, and `TermtraceError` or a subclass when the
        session cannot be attached or the recorder rejects the click (with `strict`, when the
        application does not report mouse input).
        """
        return await asyncio.to_thread(
            lambda: self._sync.pointer_click(column, row, button, strict)
        )

    async def pointer_scroll(
        self,
        column: builtins.int,
        row: builtins.int,
        lines: builtins.int | None = None,
        strict: builtins.bool | None = None,
    ) -> None:
        r"""Legacy scroll adapter over `pointer().scroll`; returns `None`.

        # Errors

        Raises `TermtraceError` or a subclass when the session cannot be attached or the recorder
        rejects the scroll (with `strict`, when the application does not report mouse input).
        """
        return await asyncio.to_thread(
            lambda: self._sync.pointer_scroll(column, row, lines, strict)
        )

    async def pointer_focus(
        self, focused: builtins.bool, strict: builtins.bool | None = None
    ) -> None:
        r"""Legacy focus adapter over `pointer().focus`; returns `None`.

        # Errors

        Raises `TermtraceError` or a subclass when the session cannot be attached or the recorder
        rejects the focus report (with `strict`, when the application does not report focus).
        """
        return await asyncio.to_thread(
            lambda: self._sync.pointer_focus(focused, strict)
        )

    async def resize(
        self,
        cols: builtins.int,
        rows: builtins.int,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.ControlAcknowledgement:
        r"""Resize the child PTY to `cols` by `rows` cells.

        # Errors

        Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.resize(
                cols, rows, request_id=request_id, if_state=if_state
            ),
        )

    async def signal(
        self,
        signal: types.ProcessSignal,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.ControlAcknowledgement:
        r"""Send `interrupt`, `terminate`, `hangup`, `user1`, or `user2` to the child process group.

        # Errors

        Raises `ValueError` for an unknown signal and `TermtraceError` or a subclass with
        `request_id` when the recorder rejects it.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.signal(signal, request_id=request_id, if_state=if_state),
        )

    async def mark(
        self,
        label: builtins.str,
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
    ) -> types.MarkOutcome:
        r"""Append a named marker to the canonical event stream.

        # Errors

        Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.mark(label, request_id=request_id, if_state=if_state),
        )

    async def send(
        self,
        steps: typing.Sequence[types.SendStepSpecInput],
        *,
        request_id: typing.Any | None = None,
        if_state: builtins.str | None = None,
        intent: builtins.str | None = None,
        pace_ms: builtins.int | None = None,
        verbose: builtins.bool = False,
        until: types.ConditionSpecInput | None = None,
        detail: types.FrameDetail | None = None,
        region: types.ScreenRegion | None = None,
        report_changes: builtins.bool = False,
    ) -> typing.Any:
        r"""Apply ordered steps; each is one mutation whose identity derives from the send's, so
        retrying the whole send with its `request_id` replays without duplicates. `if_state`
        guards only the first step.

        # Errors

        Raises `ValueError` for malformed steps or options, and on a failed step its
        `TermtraceError` with `step`, its derived `request_id`, and `send_request_id`; earlier
        steps were applied.
        """
        return await offload_mutation(
            self._pending,
            lambda: self._sync.send(
                steps,
                request_id=request_id,
                if_state=if_state,
                intent=intent,
                pace_ms=pace_ms,
                verbose=verbose,
                until=until,
                detail=detail,
                region=region,
                report_changes=report_changes,
            ),
        )

    async def status(self) -> types.SessionStatus:
        r"""Live status `{"kind": "live", ...}` or completion `{"kind": "completed", ...}`.

        # Errors

        Raises `TermtraceError` or a subclass when the session cannot be attached or read.
        """
        return await asyncio.to_thread(lambda: self._sync.status())

    async def save(
        self,
        destination: builtins.str,
        formats: typing.Sequence[builtins.str] | None = None,
        state_hash: builtins.str | None = None,
        include_hidden: builtins.bool | None = None,
    ) -> types.SaveOutcome:
        r"""Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.

        # Errors

        Raises `ValueError` when a format name is unknown, and `TermtraceError` or a subclass when
        the state cannot be resolved or rendered or the destination cannot be published.
        """
        return await asyncio.to_thread(
            lambda: self._sync.save(destination, formats, state_hash, include_hidden)
        )

    async def export_asciicast(self, output_path: builtins.str) -> None:
        r"""Export session terminal playback as an asciicast v2 file.

        # Errors

        Raises `TermtraceError` or a subclass when the event log cannot be read or the cast cannot
        be written.
        """
        return await asyncio.to_thread(lambda: self._sync.export_asciicast(output_path))

    async def create_bundle(self, output_path: builtins.str) -> None:
        r"""Create a compressed evidence bundle (.tar.zst) for archiving or forensics.

        # Errors

        Raises `TermtraceError` or a subclass when the bundle cannot be created or published.
        """
        return await asyncio.to_thread(lambda: self._sync.create_bundle(output_path))

    async def diff(
        self, from_hash: builtins.str, to_hash: builtins.str
    ) -> types.SemanticDiff:
        r"""Compute semantic difference between two presented states and return JSON.

        # Errors

        Raises `TermtraceError` or a subclass when either state hash cannot be resolved or the
        session's evidence cannot be validated or replayed, and `ValueError` when the difference
        cannot be serialized.
        """
        return await asyncio.to_thread(lambda: self._sync.diff(from_hash, to_hash))

    async def read_output(
        self, offset: builtins.int | None = None, limit: builtins.int | None = None
    ) -> tuple[builtins.list[builtins.int], builtins.int, builtins.bool]:
        r"""Read raw child PTY output bytes using an optional byte offset cursor.

        # Errors

        Raises `TermtraceError` or a subclass when the output page cannot be read.
        """
        return await asyncio.to_thread(lambda: self._sync.read_output(offset, limit))

    async def wait_for_output(
        self, pattern: builtins.str, timeout_secs: builtins.float | None = None
    ) -> builtins.bool:
        r"""Wait for exact child output bytes to appear in the canonical stream.

        # Errors

        Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range, and
        `TermtraceError` or a subclass when `pattern` is empty, the event log is invalid, the child
        exits or the session completes before the bytes appear, or the timeout elapses.
        """
        return await asyncio.to_thread(
            lambda: self._sync.wait_for_output(pattern, timeout_secs)
        )

    async def read_events(
        self, cursor: builtins.int | None = None, limit: builtins.int | None = None
    ) -> builtins.list[types.Event]:
        r"""Read canonical event records as pretty JSON starting at an optional event sequence.

        # Errors

        Raises `TermtraceError` or a subclass when the event page cannot be read or serialized.
        """
        return await asyncio.to_thread(lambda: self._sync.read_events(cursor, limit))

    async def explain(
        self,
        target_hash: builtins.str | None = None,
        context: builtins.int | None = None,
    ) -> types.ExplainReport:
        r"""Explain VT and semantic evidence around a target state point.

        # Errors

        Raises `TermtraceError` or a subclass when the target state cannot be resolved, the
        session's evidence cannot be validated or replayed, or the report cannot be serialized.
        """
        return await asyncio.to_thread(lambda: self._sync.explain(target_hash, context))

    async def diagnose(
        self, broad_change_percent: builtins.int | None = None
    ) -> types.DiagnoseReport:
        r"""Run conservative evidence-backed rendering diagnostics on the session.

        # Errors

        Raises `TermtraceError` or a subclass when visual or canonical evidence fails validation or
        replay, or the report cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.diagnose(broad_change_percent)
        )

    async def last_action(
        self, broad_change_percent: builtins.int | None = None
    ) -> types.LastActionAnalysis:
        r"""Perform full triage analysis on the most recent canonical action.

        # Errors

        Raises `TermtraceError` or a subclass when the most recent action cannot be analyzed from
        validated evidence or the report cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.last_action(broad_change_percent)
        )

    async def timeline_page(
        self,
        offset: builtins.int | None = None,
        limit: builtins.int | None = None,
        include_hidden: builtins.bool | None = None,
        include_controls: builtins.bool | None = None,
    ) -> types.TimelinePage:
        r"""Build a bounded page of timeline entries as pretty JSON.

        # Errors

        Raises `TermtraceError` or a subclass when the session's evidence cannot be read, validated,
        or replayed, or the page cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.timeline_page(
                offset, limit, include_hidden, include_controls
            )
        )


class AsyncSystem(Wrapper):
    r"""Asynchronous System: System capability probes, versions, and the public schema catalog."""

    def __init__(self) -> None:
        self._sync = _native.System()
        self._pending = set()

    async def capabilities(
        self, cancel: CancelToken | None = None
    ) -> types.CapabilityReport:
        r"""Probe capabilities; unavailable tools are reported as blocked, never skipped.

        Cancelling `cancel` stops the native capture probe early; it then reports blocked.
        """

        def call(token: CancelToken) -> typing.Any:
            return self._sync.capabilities(token)

        return await run_cancellable(call, cancel)

    async def versions(self) -> types.Versions:
        r"""Build and component versions."""
        return await asyncio.to_thread(lambda: self._sync.versions())

    async def schemas(self) -> builtins.list[builtins.str]:
        r"""Names in the public schema catalog.
        # Errors
        Propagates catalog registration failures.
        """
        return await asyncio.to_thread(lambda: self._sync.schemas())

    async def schema(self, name: builtins.str) -> types.AnyValue:
        r"""One public JSON Schema.
        # Errors
        Unknown schemas and generation failures propagate.
        """
        return await asyncio.to_thread(lambda: self._sync.schema(name))


class AsyncTermtrace(Wrapper):
    r"""Asynchronous Termtrace: Primary entry point for launching and managing Termtrace recordings in Python."""

    def __init__(
        self,
        binary: builtins.str | None = None,
        evidence_root: builtins.str | None = None,
    ) -> None:
        self._sync = _native.Termtrace(binary, evidence_root)
        self._pending = set()

    def system(self) -> AsyncSystem:
        r"""Local typed system service, without starting any probe yet."""
        return AsyncSystem._wrap(self._sync.system())

    def instrumentation(
        self, source: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> AsyncInstrumentation:
        r"""Instrumentation bound to one live session."""
        return AsyncInstrumentation._wrap(self._sync.instrumentation(source))

    def tmux(
        self,
        root: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        tmux: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        socket: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        socket_name: builtins.bool = False,
    ) -> AsyncTmux:
        r"""Aggregate tmux recording with an explicit root, tmux executable, and socket."""
        return AsyncTmux._wrap(self._sync.tmux(root, tmux, socket, socket_name))

    def registry(
        self, root: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None
    ) -> AsyncRegistry:
        r"""Registry operations at an explicit or the default state root."""
        return AsyncRegistry._wrap(self._sync.registry(root))

    def analysis(
        self, source: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> AsyncDiagnostics:
        r"""Replay analysis of one canonical session."""
        return AsyncDiagnostics._wrap(self._sync.analysis(source))

    def evidence(
        self, source: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> AsyncEvidence:
        r"""Evidence artifacts and integrity operations for one canonical session."""
        return AsyncEvidence._wrap(self._sync.evidence(source))

    def native(
        self, source: str | bytes | os.PathLike[str] | os.PathLike[bytes]
    ) -> AsyncNative:
        r"""Native replay operations for one canonical session."""
        return AsyncNative._wrap(self._sync.native(source))

    async def run_scenarios(
        self,
        scenarios: typing.Sequence[str | bytes | os.PathLike[str] | os.PathLike[bytes]],
        *,
        failure_root: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        evidence_root: str
        | bytes
        | os.PathLike[str]
        | os.PathLike[bytes]
        | None = None,
        jobs: builtins.int = 1,
        shard_index: builtins.int | None = None,
        shard_count: builtins.int | None = None,
        update_snapshots: builtins.bool = False,
        reruns: builtins.int = 0,
        fail_on_flaky: builtins.bool = False,
    ) -> types.ScenarioSuiteReport:
        r"""Run scenario documents as one suite with this client's binary. A document with a
        `matrix` expands to one labelled case per combination; with `shard_index` and
        `shard_count`, case `i` of the sorted expansion runs only when
        `i % shard_count == shard_index`. Up to `jobs` cases run at once, each in a fresh session
        under `evidence_root` (default: this client's evidence root). Failure artifacts go under
        `failure_root` and snapshot baselines in its sibling `snapshots` directory, replaced when
        `update_snapshots` is set. A failed case runs again up to `reruns` times; a later pass
        makes it `flaky`, which fails CI only with `fail_on_flaky`.

        # Errors

        Raises `TermtraceError` when the options are invalid, a document cannot be read or
        expanded, or the shard is empty. Case failures stay in the report.
        """
        return await asyncio.to_thread(
            lambda: self._sync.run_scenarios(
                scenarios,
                failure_root=failure_root,
                evidence_root=evidence_root,
                jobs=jobs,
                shard_index=shard_index,
                shard_count=shard_count,
                update_snapshots=update_snapshots,
                reruns=reruns,
                fail_on_flaky=fail_on_flaky,
            )
        )

    async def record(
        self,
        command: typing.Sequence[builtins.str],
        rows: builtins.int | None = None,
        cols: builtins.int | None = None,
        env: typing.Mapping[builtins.str, builtins.str] | None = None,
        cwd: builtins.str | None = None,
        visual_profile: types.VisualProfile | None = None,
    ) -> AsyncRecording:
        r"""Launch a new recorded command session. `visual_profile` is committed into the session;
        its native section, when present, is resolved before the command starts.

        # Errors

        Raises `ValueError` for a malformed profile, and `TermtraceError` or a subclass when the
        profile is invalid or does not resolve, or the command cannot be launched and recorded.
        """
        return AsyncRecording._wrap(
            await launch(
                lambda: self._sync.record(command, rows, cols, env, cwd, visual_profile)
            )
        )

    async def preflight_visual_profile(
        self,
        profile: types.VisualProfile,
        cols: builtins.int | None = None,
        rows: builtins.int | None = None,
    ) -> types.VisualProfileReport:
        r"""Validate a visual profile and report the identity of everything a launch with it would
        use. A semantic-only profile runs no native program; a native section is resolved by
        Ghostty and the `cols` by `rows` grid is solved at the profile's DPI. Advisory only.

        # Errors

        Raises `ValueError` for a malformed profile, and `TermtraceError` or a subclass with the
        visual-profile failure code when it is invalid or does not resolve.
        """
        return await asyncio.to_thread(
            lambda: self._sync.preflight_visual_profile(profile, cols, rows)
        )

    async def video_plan(
        self,
        session: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        edit: editing.Edit | types.EditSourceInput | str | os.PathLike[str],
        *,
        renderer: types.VideoRenderer,
        fps: builtins.int | None = None,
        semantic_raster_profile: types.SemanticRasterProfile | None = None,
    ) -> types.PresentationPlan:
        r"""Resolve the presentation plan of `edit` over a completed `session` without rendering.
        `renderer` is `native` (verified Ghostty pixels, captured first when missing; never
        semantic) or `semantic`; `fps` defaults to 30.

        # Errors

        Raises `TermtraceError` with an edit failure `code` and `details` for a rejected edit,
        and for blocked capabilities, invalid evidence, or capture failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.video_plan(
                session,
                edit,
                renderer=renderer,
                fps=fps,
                semantic_raster_profile=semantic_raster_profile,
            )
        )

    async def video_preview(
        self,
        session: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        edit: editing.Edit | types.EditSourceInput | str | os.PathLike[str],
        directory: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        *,
        renderer: types.VideoRenderer,
        fps: builtins.int | None = None,
        semantic_raster_profile: types.SemanticRasterProfile | None = None,
        preview: types.PreviewOptions | None = None,
    ) -> types.VideoPreview:
        r"""Render a contact sheet and low-resolution review video of `edit` into the new
        `directory`. Options are those of `video_plan`, plus `preview` storyboard settings.

        # Errors

        Raises like `video_plan`, and when the directory exists or rendering fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.video_preview(
                session,
                edit,
                directory,
                renderer=renderer,
                fps=fps,
                semantic_raster_profile=semantic_raster_profile,
                preview=preview,
            )
        )

    async def video_render(
        self,
        session: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        edit: editing.Edit | types.EditSourceInput | str | os.PathLike[str],
        out: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        *,
        renderer: types.VideoRenderer,
        fps: builtins.int | None = None,
        semantic_raster_profile: types.SemanticRasterProfile | None = None,
        gif_loop: types.GifLoop | None = None,
        overwrite: builtins.bool = False,
    ) -> types.VideoRender:
        r"""Render `edit` to `out`, whose lowercase `.mp4`, `.gif`, or `.webm` extension selects
        the format, keeping a manifested `.termtrace` sidecar beside it. An existing output
        fails unless `overwrite` is set for an MP4; GIF and `WebM` are never replaced.

        # Errors

        Raises like `video_plan`, and when the output exists or rendering fails.
        """
        return await asyncio.to_thread(
            lambda: self._sync.video_render(
                session,
                edit,
                out,
                renderer=renderer,
                fps=fps,
                semantic_raster_profile=semantic_raster_profile,
                gif_loop=gif_loop,
                overwrite=overwrite,
            )
        )

    async def launch(
        self,
        command: typing.Sequence[builtins.str],
        options: types.LaunchOptionsInput | None = None,
    ) -> AsyncRecording:
        r"""Launch a recorded command with typed options: `rows`, `cols`, `cwd`, `environment`
        (`base`, `set` name to bytes, `copy`, `unset`, `color`), `startup_timeout_ms`,
        `output_byte_budget`, `max_duration_ms`, `idle_timeout_ms`, and `visual_profile`.

        Returns once the recorder answers or the session already completed. A cleared environment
        still receives the reserved terminal variables.

        # Errors

        Raises `ValueError` for unknown or malformed options, and `TermtraceError` or a subclass when
        the options are invalid or the command cannot be launched and recorded.
        """
        return AsyncRecording._wrap(
            await launch(lambda: self._sync.launch(command, options))
        )

    async def record_original(
        self,
        command: typing.Sequence[builtins.str],
        options: types.OriginalLaunchOptionsInput | None = None,
        cancel: CancelToken | None = None,
    ) -> AsyncOriginalRecording:
        r"""Launch `command` as an original recording: it runs in a private Ghostty display whose
        pixels are captured from before its first byte.

        `options`: `grid` (`cols`, `rows`) or `viewport` (`width`, `height`), `canvas`, `fps`
        (24, 30 or 60), `cwd`, `environment`, `ghostty_config` or `visual_profile`, `alias`, `state_root`,
        `startup_timeout_ms`, `output_byte_budget`, `max_duration_ms` and `idle_timeout_ms`.
        Cancelling `cancel` tears startup down within a bounded deadline.

        # Errors

        Raises `ValueError` for malformed options, `OriginalCaptureFailedError` when acquisition
        fails or is cancelled, and `TermtraceError` when native capture is unavailable or startup
        misses its deadline.
        """

        def call(token: CancelToken) -> typing.Any:
            return self._sync.record_original(command, options, token)

        return AsyncOriginalRecording._wrap(await run_cancellable(call, cancel))

    async def attach_original(
        self, session_path: builtins.str
    ) -> AsyncOriginalRecording:
        r"""Attach to an original recording without owning its capture supervisor.

        # Errors

        Raises `TermtraceError` when the session or its capture status is missing or invalid.
        """
        return AsyncOriginalRecording._wrap(
            await asyncio.to_thread(lambda: self._sync.attach_original(session_path))
        )

    async def attach_original_target(
        self, target: typing.Any, state_root: typing.Any | None = None
    ) -> AsyncOriginalRecording:
        r"""Attach to an original recording named by path or registry alias, without owning its
        capture supervisor. `str` and `bytes` follow the `path:`/`name:` rules; an
        `os.PathLike` is an exact path. Aliases resolve under `state_root`, or the default
        registry.

        # Errors

        Raises `TermtraceError` when the target does not resolve, or the session or its capture
        status is missing, invalid, not owner-only, or published for another session.
        """
        return AsyncOriginalRecording._wrap(
            await asyncio.to_thread(
                lambda: self._sync.attach_original_target(target, state_root)
            )
        )

    async def attach_target(self, target: builtins.str) -> AsyncSession:
        r"""Attach to a session named by path or registry alias; `path:` and `name:` prefixes are
        explicit.

        # Errors

        Raises `TermtraceError` or a subclass when the target does not resolve or cannot be
        attached.
        """
        return AsyncSession._wrap(
            await asyncio.to_thread(lambda: self._sync.attach_target(target))
        )

    async def attach(self, session_path: builtins.str) -> AsyncSession:
        r"""Attach to an existing session directory.

        # Errors

        Raises `TermtraceError` or a subclass when `session_path` is not an attachable session.
        """
        return AsyncSession._wrap(
            await asyncio.to_thread(lambda: self._sync.attach(session_path))
        )

    async def verify_bundle(
        self, bundle_path: builtins.str, source_session: builtins.str | None = None
    ) -> builtins.bool:
        r"""Verify an evidence bundle (.tar.zst or directory) and return whether it passed integrity verification.

        # Errors

        Never raises; failed verification returns `False`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.verify_bundle(bundle_path, source_session)
        )

    async def verify_session(
        self, session_path: builtins.str, level: types.VerificationLevel | None = None
    ) -> types.VerificationReport:
        r"""Verify a completed session's structure and evidence bindings offline.

        # Errors

        Raises `TermtraceError` or a subclass when the verification report cannot be serialized;
        verification findings are part of the returned report.
        """
        return await asyncio.to_thread(
            lambda: self._sync.verify_session(session_path, level)
        )

    async def recover_session(
        self, session_path: builtins.str, destination: builtins.str
    ) -> types.RecoveryReport:
        r"""Recover the longest integrity-checked event prefix from a damaged session.

        # Errors

        Raises `TermtraceError` or a subclass when no integrity-checked prefix can be recovered and
        published to `destination`, or the report cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.recover_session(session_path, destination)
        )

    async def registry_list(
        self, include_all: builtins.bool | None = None
    ) -> builtins.list[types.RegistryListEntry]:
        r"""List registered sessions in the default user registry.

        # Errors

        Raises `TermtraceError` or a subclass when the default registry cannot be opened or read, or
        the entries cannot be serialized.
        """
        return await asyncio.to_thread(lambda: self._sync.registry_list(include_all))

    async def registry_resolve(self, name: builtins.str) -> builtins.str | None:
        r"""Resolve a friendly session name to its registered session path.

        # Errors

        Raises `TermtraceError` or a subclass when the default registry cannot be opened; an
        unresolvable name returns `None`.
        """
        return await asyncio.to_thread(lambda: self._sync.registry_resolve(name))

    async def registry_register(
        self, name: builtins.str, session_path: builtins.str
    ) -> types.RegistryEntry:
        r"""Register a friendly name for an existing canonical session directory.

        # Errors

        Raises `TermtraceError` or a subclass when the default registry cannot be opened, `name` is
        invalid or already registered, `session_path` is not a registrable session, or the entry
        cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.registry_register(name, session_path)
        )

    async def registry_remove(self, name: builtins.str) -> builtins.bool:
        r"""Remove a friendly name binding.

        # Errors

        Raises `TermtraceError` or a subclass when the default registry cannot be opened; a failed
        removal returns `False`.
        """
        return await asyncio.to_thread(lambda: self._sync.registry_remove(name))

    async def registry_gc(
        self,
        older_than_days: builtins.int | None = None,
        keep_latest: builtins.int | None = None,
        max_completed_bytes: builtins.int | None = None,
        apply: builtins.bool | None = None,
    ) -> types.GarbageCollectionReport:
        r"""Plan or execute garbage collection over completed sessions.

        # Errors

        Raises `TermtraceError` or a subclass when the default registry cannot be opened, garbage
        collection cannot be planned or applied, or the report cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.registry_gc(
                older_than_days, keep_latest, max_completed_bytes, apply
            )
        )

    async def tmux_bundle(
        self, manifest_path: builtins.str, destination: builtins.str
    ) -> types.TmuxStreamingBundleManifest:
        r"""Bundle an aggregate tmux recording into an evidentiary streaming bundle.

        # Errors

        Raises `TermtraceError` or a subclass when the aggregate tmux recording cannot be bundled or
        published, or the report cannot be serialized.
        """
        return await asyncio.to_thread(
            lambda: self._sync.tmux_bundle(manifest_path, destination)
        )

    async def tmux_verify_bundle(
        self, bundle_directory: builtins.str, manifest_path: builtins.str
    ) -> builtins.bool:
        r"""Verify an aggregate tmux streaming bundle against its manifest.

        # Errors

        Never raises; failed verification returns `False`.
        """
        return await asyncio.to_thread(
            lambda: self._sync.tmux_verify_bundle(bundle_directory, manifest_path)
        )

    async def probe_capabilities(
        self, cancel: CancelToken | None = None
    ) -> types.CapabilityReport:
        r"""Probe runtime system capabilities across engine, media, and evidence signers.

        Cancelling `cancel` stops the native capture probe early; it then reports blocked.

        # Errors

        Raises `TermtraceError` or a subclass only when the capability report cannot be serialized.
        """

        def call(token: CancelToken) -> typing.Any:
            return self._sync.probe_capabilities(token)

        return await run_cancellable(call, cancel)


class AsyncTmux(Wrapper):
    r"""Asynchronous Tmux: Aggregate tmux recording through the shared tmux service."""

    def __init__(
        self,
        root: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        binary: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        tmux: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        socket: str | bytes | os.PathLike[str] | os.PathLike[bytes] | None = None,
        socket_name: builtins.bool = False,
    ) -> None:
        self._sync = _native.Tmux(root, binary, tmux, socket, socket_name)
        self._pending = set()

    async def record(
        self, options: types.TmuxRecordConfigurationInput
    ) -> types.TmuxRecordingStart:
        r"""Start aggregate recording.
        # Errors
        Rejects invalid settings and propagates setup and track admission failures.
        """
        return await asyncio.to_thread(lambda: self._sync.record(options))

    async def status(self) -> types.TmuxStatusReport:
        r"""Server and integration status.
        # Errors
        Propagates connection and malformed integration failures.
        """
        return await asyncio.to_thread(lambda: self._sync.status())

    async def recording_status(self, name: builtins.str) -> types.TmuxRecordingManifest:
        r"""Lifecycle of one aggregate recording.
        # Errors
        Propagates manifest, catalog, and identity errors.
        """
        return await asyncio.to_thread(lambda: self._sync.recording_status(name))

    async def track(
        self,
        manifest: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        pane: builtins.str,
        session: str | bytes | os.PathLike[str] | os.PathLike[bytes],
    ) -> None:
        r"""Register one pane session as a track of an aggregate recording.
        # Errors
        Propagates identity and track failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.track(manifest, pane, session)
        )

    async def stop(
        self,
        name: builtins.str,
        wait: builtins.bool = True,
        timeout_ms: builtins.float | None = None,
    ) -> types.TmuxRecordingManifest:
        r"""Stop the canonical tracks of one aggregate recording.
        # Errors
        Track and identity failures propagate; prior effects are not rolled back.
        """
        return await asyncio.to_thread(lambda: self._sync.stop(name, wait, timeout_ms))

    async def events(
        self,
        name: builtins.str,
        from_sequence: builtins.int = 1,
        limit: builtins.int = 100,
    ) -> types.TmuxEvents:
        r"""One aggregate history page.
        # Errors
        Propagates manifest and journal errors and invalid paging bounds.
        """
        return await asyncio.to_thread(
            lambda: self._sync.events(name, from_sequence, limit)
        )

    async def catalog(
        self,
        name: builtins.str,
        gaps: builtins.bool = False,
        from_sequence: builtins.int = 1,
        limit: builtins.int = 100,
        active_only: builtins.bool = False,
    ) -> types.TmuxCatalog:
        r"""One track or coverage-gap catalog page.
        # Errors
        Propagates catalog, manifest, and paging failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.catalog(name, gaps, from_sequence, limit, active_only)
        )

    async def bundle(
        self,
        manifest: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
    ) -> types.TmuxStreamingBundleManifest:
        r"""Bundle an aggregate recording.
        # Errors
        Propagates mismatched or incomplete source and publication failures.
        """
        return await asyncio.to_thread(lambda: self._sync.bundle(manifest, destination))

    async def verify_bundle(
        self,
        manifest: str | bytes | os.PathLike[str] | os.PathLike[bytes],
        destination: str | bytes | os.PathLike[str] | os.PathLike[bytes],
    ) -> types.TmuxStreamingBundleManifest:
        r"""Verify an aggregate bundle against its manifest.
        # Errors
        Propagates mismatched source and verification failures.
        """
        return await asyncio.to_thread(
            lambda: self._sync.verify_bundle(manifest, destination)
        )
