"""Ordinary-test assertions and owned lifecycle adapters backed by shared Rust services.

Masks are comparison exclusions, NOT redaction. Evidence remains unmasked.
"""

from __future__ import annotations

import asyncio
import functools
import logging
import tempfile
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any, TypeVar

if TYPE_CHECKING:
    from typing_extensions import Self

    from termtrace.aio import AsyncRecording
else:
    Self = TypeVar("Self")

from termtrace import _termtrace as _native
from termtrace._termtrace import (
    CancelToken,
    ExpectationMismatchError,
    Recording,
    ScreenFrame,
    Termtrace,
    assert_count,
    assert_cursor,
    assert_exists,
    assert_match_style,
    assert_match_text,
    assert_neutral_region,
    assert_no_presented_transient,
    assert_no_wrapped_rows,
    assert_not_exists,
    assert_screen_text,
    masked_snapshot,
)
from termtrace.types import (
    EvidencePolicy,
    FinalizationReport,
    MaskedSnapshotOptionsInput,
    SecondaryFailure,
    SnapshotProfile,
    TestOutcome,
)

__all__ = [
    "AsyncTerminalTestContext",
    "ExpectationMismatchError",
    "FinalizationError",
    "FinalizationReport",
    "MaskedSnapshotOptionsInput",
    "SecondaryFailure",
    "TerminalTestContext",
    "assert_count",
    "assert_cursor",
    "assert_exists",
    "assert_masked_snapshot_file",
    "assert_match_style",
    "assert_match_text",
    "assert_neutral_region",
    "assert_no_presented_transient",
    "assert_no_wrapped_rows",
    "assert_not",
    "assert_not_exists",
    "assert_screen_text",
    "assert_snapshot_file",
    "async_terminal_test",
    "masked_snapshot",
    "normalized_snapshot",
    "terminal_test",
]

_logger = logging.getLogger(__name__)


class FinalizationError(RuntimeError):
    """A passing body was followed by cleanup or evidence verification failure."""

    def __init__(self, report: FinalizationReport) -> None:
        super().__init__(
            f"Termtrace finalization failed: {report['secondary_failures']}"
        )
        self.report = report


def normalized_snapshot(
    frame: ScreenFrame,
    *,
    profile: SnapshotProfile = "text",
    include_cursor: bool = False,
    include_modes: bool = False,
) -> str:
    """Stable JSON of this exact capture; no I/O, retries or identity rewriting."""
    return _native.normalized_snapshot(frame, profile, include_cursor, include_modes)


def assert_snapshot_file(
    frame: ScreenFrame,
    path: str | Path,
    *,
    profile: SnapshotProfile = "text",
    include_cursor: bool = False,
    include_modes: bool = False,
    update: bool = False,
) -> None:
    """Compare a reviewed baseline; updates require explicit opt-in."""
    _native.assert_snapshot_file(
        frame, str(path), profile, include_cursor, include_modes, update
    )


def assert_masked_snapshot_file(
    frame: ScreenFrame,
    path: str | Path,
    options: MaskedSnapshotOptionsInput,
    *,
    update: bool = False,
) -> None:
    """Compare a separate masked baseline. Masks are NOT redaction."""
    _native.assert_masked_snapshot_file(frame, str(path), options, update)


def assert_not(expectation: Callable[[], object]) -> None:
    """Negate only an expectation mismatch, never query/operational failures."""
    try:
        expectation()
    except ExpectationMismatchError:
        return
    _native.fail_negation()


def _diagnostics(error: BaseException, report: FinalizationReport) -> None:
    try:
        if not hasattr(error, "termtrace"):
            attribute = "termtrace"
            setattr(error, attribute, report)
    except BaseException:  # noqa: BLE001 - attachment must never replace BaseException/cancellation
        _logger.error("Termtrace finalization report: %s", report)
    if report["retained"] or report["secondary_failures"]:
        _logger.error("Termtrace finalization report: %s", report)


class TerminalTestContext:
    """Own only recordings launched through this context. Explicit finalize is exactly once.

    The callback/context boundary cannot see later assertions. Use Always for later failures.
    An explicit finalize commits the supplied outcome; no rollback is implied.
    """

    def __init__(
        self,
        *,
        binary: str | None = None,
        evidence_root: str | None = None,
        evidence_policy: EvidencePolicy = "on_failure",
        finalization_timeout_secs: float = 10,
        _client: Termtrace | None = None,
        _owned_root: str | None = None,
    ) -> None:
        import math

        if evidence_policy not in ("on_failure", "always", "manual"):
            raise ValueError("invalid evidence_policy")
        if (
            not math.isfinite(finalization_timeout_secs)
            or finalization_timeout_secs < 0
        ):
            raise ValueError("invalid finalization_timeout_secs")
        self._evidence_root = _owned_root or tempfile.mkdtemp(
            prefix="termtrace-test-", dir=evidence_root
        )
        self.termtrace = _client or Termtrace(binary, self.evidence_root)
        self._evidence_policy = evidence_policy
        self._finalization_timeout_secs = finalization_timeout_secs
        self.report: FinalizationReport | None = None
        self._recordings: list[Recording] = []
        self._closed = False
        self._retain = CancelToken()

    @property
    def evidence_root(self) -> str:
        return self._evidence_root

    @property
    def evidence_policy(self) -> EvidencePolicy:
        return self._evidence_policy

    @property
    def finalization_timeout_secs(self) -> float:
        return self._finalization_timeout_secs

    def record(
        self,
        command: Sequence[str],
        *,
        rows: int | None = None,
        cols: int | None = None,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> Recording:
        """Launch and track an owned recording; direct client launches are not owned."""
        if self._closed:
            raise RuntimeError("terminal test is finalized")
        rec = self.termtrace.record(
            list(command), rows, cols, dict(env) if env is not None else None, cwd
        )
        self._recordings.append(rec)
        return rec

    def finalize(self, outcome: TestOutcome = "passed") -> FinalizationReport:
        """Finalize, verify and apply retention once in shared Rust."""
        if self.report is None:
            self._closed = True
            self.report = _native.finalize_recordings(
                self._recordings,
                self.evidence_root,
                outcome,
                self.evidence_policy,
                self.finalization_timeout_secs,
                self._retain,
            )
        return self.report

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: object,
    ) -> None:
        try:
            report = self.finalize("error" if exc is not None else "passed")
        except BaseException as cleanup:
            if exc is None:
                raise
            _logger.error(
                "Termtrace cleanup failed; evidence: %s",
                self.evidence_root,
                exc_info=cleanup,
            )
            return
        if exc is not None:
            _diagnostics(exc, report)
        elif report["secondary_failures"]:
            raise FinalizationError(report)


class AsyncTerminalTestContext:
    """Off-loop lifecycle adapter owning admitted launches and late-launch cleanup.

    Repeated cancellation never interrupts finalization. Cancellation does not roll back input.
    Native launch, stop and force-close have their own bounded waits; runner kill cannot be handled.
    """

    def __init__(self, context: TerminalTestContext) -> None:
        self._context = context
        self._launches: set[asyncio.Future[Any]] = set()
        self._recordings: list[AsyncRecording] = []
        self._finalization: asyncio.Task[FinalizationReport] | None = None
        self._closing = False

    @property
    def evidence_root(self) -> str:
        return self._context.evidence_root

    @property
    def report(self) -> FinalizationReport | None:
        return self._context.report

    async def record(
        self,
        command: Sequence[str],
        *,
        rows: int | None = None,
        cols: int | None = None,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> AsyncRecording:
        """An admitted launch is owned even when its await is cancelled."""
        from termtrace.aio import AsyncRecording

        if self._closing:
            raise RuntimeError("terminal test is finalized")
        call = functools.partial(
            self._context.record, command, rows=rows, cols=cols, env=env, cwd=cwd
        )
        worker = asyncio.get_running_loop().run_in_executor(None, call)
        self._launches.add(worker)
        # Do not discard completed workers until teardown observes launch errors.
        rec = await asyncio.shield(worker)
        result = AsyncRecording._wrap(rec)
        self._recordings.append(result)
        return result

    async def _finish(self, outcome: TestOutcome) -> FinalizationReport:
        launch_failed = False
        if self._launches:
            results = await asyncio.gather(*self._launches, return_exceptions=True)
            launch_failed = any(isinstance(result, BaseException) for result in results)
        waits = [asyncio.create_task(rec.wait_pending()) for rec in self._recordings]
        pending_mutation = False
        if waits:
            _, pending = await asyncio.wait(
                waits, timeout=self._context.finalization_timeout_secs
            )
            pending_mutation = bool(pending)
            for waiter in pending:
                waiter.cancel()  # cancels only our join; admitted mutations keep their own workers
        if pending_mutation:
            outcome = "timed_out"
        report = await asyncio.to_thread(
            self._context.finalize, "error" if launch_failed else outcome
        )
        if pending_mutation:
            report["secondary_failures"].append(
                {
                    "phase": "pending_mutation",
                    "message": "owned worker join deadline expired; input, if any, is not rolled back",
                    "truncated": False,
                }
            )
        if launch_failed:
            report["secondary_failures"].append(
                {
                    "phase": "session_launch",
                    "message": "an owned launch failed",
                    "truncated": False,
                }
            )
        return report

    async def finalize(self, outcome: TestOutcome = "passed") -> FinalizationReport:
        """Shield one off-loop finalization, waiting through repeated cancellation."""
        self._closing = True
        if self._finalization is None:
            self._finalization = asyncio.create_task(self._finish(outcome))
        cancelled: asyncio.CancelledError | None = None
        while True:
            try:
                result = await asyncio.shield(self._finalization)
                break
            except asyncio.CancelledError as error:
                self._context._retain.cancel()
                cancelled = cancelled or error
                if self._finalization.done():
                    result = self._finalization.result()
                    break
                continue
        if cancelled is not None:
            raise cancelled
        return result

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: object,
    ) -> None:
        try:
            report = await self.finalize("error" if exc is not None else "passed")
        except BaseException as cleanup:
            if exc is None:
                raise
            if self.report is not None:
                _diagnostics(exc, self.report)
            _logger.error(
                "Termtrace cleanup interrupted; evidence: %s",
                self.evidence_root,
                exc_info=cleanup,
            )
            return
        if exc is not None:
            _diagnostics(exc, report)
        elif report["secondary_failures"]:
            raise FinalizationError(report)


def terminal_test(
    *,
    binary: str | None = None,
    evidence_root: str | None = None,
    evidence_policy: EvidencePolicy = "on_failure",
    finalization_timeout_secs: float = 10,
) -> TerminalTestContext:
    return TerminalTestContext(
        binary=binary,
        evidence_root=evidence_root,
        evidence_policy=evidence_policy,
        finalization_timeout_secs=finalization_timeout_secs,
    )


def async_terminal_test(
    *,
    binary: str | None = None,
    evidence_root: str | None = None,
    evidence_policy: EvidencePolicy = "on_failure",
    finalization_timeout_secs: float = 10,
) -> AsyncTerminalTestContext:
    return AsyncTerminalTestContext(
        terminal_test(
            binary=binary,
            evidence_root=evidence_root,
            evidence_policy=evidence_policy,
            finalization_timeout_secs=finalization_timeout_secs,
        )
    )
