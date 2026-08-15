"""Stable, runner-neutral public error contracts."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from termtrace.screen.frame import ScreenFrame
    from termtrace.screen.queries import CellRange
    from termtrace.session.actions import ActionResult, StableCapture

MAX_DIAGNOSTIC_CHARS = 512
MAX_DIAGNOSTIC_MATCHES = 8


def bounded_text(value: str, maximum: int = MAX_DIAGNOSTIC_CHARS) -> str:
    if len(value) <= maximum:
        return value
    if maximum == 0:
        return ""
    return f"{value[: maximum - 1]}…"


class TermtraceError(RuntimeError):
    """Base class for operational client failures."""

    code = "termtrace"


class TermtraceOperationError(TermtraceError):
    """A driver operation rejected before richer code mapping was available."""

    def __init__(
        self,
        message: str,
        *,
        code: str | None = None,
        retry_safety: str = "unknown",
        uncertain: bool = False,
        evidence: str | None = None,
        request_id: str | None = None,
        details: object | None = None,
    ) -> None:
        super().__init__(bounded_text(message))
        if code is not None:
            self.code = code
        self.retry_safety = retry_safety
        self.uncertain = uncertain
        self.evidence = bounded_text(evidence) if evidence is not None else None
        self.request_id = request_id
        self.details = details


class RequestTimeoutError(TermtraceOperationError):
    """A bounded driver request timed out with an uncertain operation result."""

    code = "request_timeout"

    def __init__(self, timeout: float) -> None:
        super().__init__(
            f"driver request exceeded its {timeout:g} second deadline",
            code=self.code,
            retry_safety="unsafe",
            uncertain=True,
        )


class ValidationError(TermtraceError, ValueError):
    code = "validation"


class InvalidAtomError(ValidationError):
    code = "invalid_atom"


class InvalidDurationError(ValidationError):
    code = "invalid_duration"


class InvalidRegionError(ValidationError):
    code = "invalid_region"


class InvalidQueryError(ValidationError):
    code = "invalid_query"


class StateError(TermtraceOperationError):
    code = "state"


class StaleStateError(StateError):
    code = "stale_state"

    def __init__(
        self,
        message: str,
        *,
        expected_state_hash: str | None,
        current_state_hash: str,
        retry_safety: str,
        evidence: str | None = None,
        uncertain: bool = False,
        request_id: str | None = None,
        details: object | None = None,
    ) -> None:
        super().__init__(
            message,
            retry_safety=retry_safety,
            uncertain=uncertain,
            evidence=evidence,
            details=details,
            request_id=request_id,
        )
        self.expected_state_hash = expected_state_hash
        self.current_state_hash = current_state_hash


class BusyError(TermtraceOperationError):
    code = "busy"


class ActionBusyError(BusyError):
    code = "action_busy"


class InputSequenceBusyError(BusyError):
    code = "input_sequence_busy"


class IncompleteError(TermtraceError):
    code = "incomplete"


class PostconditionIncompleteError(IncompleteError):
    code = "postcondition_incomplete"

    def __init__(self, result: ActionResult) -> None:
        from termtrace.session.actions import IdleCompletion, ScreenCompletion

        completion = result.postcondition
        quality = (
            completion.quality
            if isinstance(completion, IdleCompletion | ScreenCompletion)
            else "unknown"
        )
        super().__init__(f"action ended with incomplete quality {quality}")
        self.result = result


class StableCaptureIncompleteError(IncompleteError):
    code = "stable_capture_incomplete"

    def __init__(self, result: StableCapture) -> None:
        super().__init__(f"stable capture ended with incomplete quality {result.quality}")
        self.result = result


class InputError(TermtraceOperationError):
    code = "input"


class PartialInputError(InputError):
    code = "partial_input"

    def __init__(
        self,
        message: str,
        *,
        completed_atoms: int,
        failed_atom_index: int,
        operation_id: str,
        bytes_written: int,
        first_event_seq: int,
        last_event_seq: int,
        resume_from_atom_index: int,
        retry_safety: str,
        uncertain: bool,
        evidence: str | None = None,
        request_id: str | None = None,
        details: object | None = None,
    ) -> None:
        super().__init__(
            message,
            retry_safety=retry_safety,
            uncertain=uncertain,
            evidence=evidence,
            details=details,
            request_id=request_id,
        )
        self.completed_atoms = completed_atoms
        self.failed_atom_index = failed_atom_index
        self.operation_id = operation_id
        self.bytes_written = bytes_written
        self.first_event_seq = first_event_seq
        self.last_event_seq = last_event_seq
        self.resume_from_atom_index = resume_from_atom_index


class ResourceLimitError(TermtraceOperationError):
    code = "resource_limit"


class ProtocolMismatchError(TermtraceError):
    code = "protocol_mismatch"


class TransportError(TermtraceError):
    code = "transport"


class NoMatchError(TermtraceError, LookupError):
    code = "no_match"

    def __init__(self, message: str, *, state_hash: str | None = None) -> None:
        detail = message
        if state_hash is not None:
            detail = f"{message}; state_hash={state_hash}"
        super().__init__(bounded_text(detail))
        self.state_hash = state_hash


class MultipleMatchesError(TermtraceError, LookupError):
    code = "multiple_matches"

    def __init__(
        self,
        count: int,
        excerpts: Sequence[str] = (),
        *,
        state_hash: str,
        ranges: Sequence[Sequence[CellRange]] = (),
    ) -> None:
        shown = tuple(bounded_text(value, 80) for value in excerpts[:8])
        super().__init__(
            f"received {count} matches in state {state_hash}, expected exactly one; "
            f"excerpts={shown!r}"
        )
        self.count = count
        self.excerpts = shown
        self.state_hash = state_hash
        self.ranges = tuple(tuple(match_ranges) for match_ranges in ranges[:8])
        self.truncated = count > len(shown) or len(ranges) > len(self.ranges)


class LocatorTimeoutError(TermtraceError, TimeoutError):
    code = "locator_timeout"

    def __init__(self, message: str, *, last_frame: ScreenFrame | None, query: str) -> None:
        super().__init__(bounded_text(message))
        self.last_frame = last_frame
        self.query = bounded_text(query)


class TermtraceProtocolError(ProtocolMismatchError):
    """Compatibility name for malformed or uncorrelated driver responses."""


__all__ = [name for name in globals() if name.endswith("Error") or name == "bounded_text"]
