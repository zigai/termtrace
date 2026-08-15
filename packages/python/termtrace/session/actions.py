"""Session action options, outcomes, and completion models."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, TypeAlias

from termtrace.duration import Duration
from termtrace.screen.frame import FrameRequest, ScreenFrame
from termtrace.transport.protocol import JsonObject

InputKind: TypeAlias = Literal["raw", "text", "keys", "paste", "focus", "pointer"]

ActionQuality: TypeAlias = Literal[
    "stable",
    "matched",
    "matched_unsettled",
    "child_completed",
    "output_closed",
    "deadline",
]
StableQuality: TypeAlias = Literal["stable", "child_completed", "output_closed", "deadline"]


@dataclass(frozen=True, slots=True)
class NonePostcondition:
    kind: Literal["none"] = "none"


@dataclass(frozen=True, slots=True)
class IdlePostcondition:
    stable_for: Duration
    timeout: Duration
    kind: Literal["idle"] = "idle"

    def __post_init__(self) -> None:
        if self.stable_for.nanoseconds == 0:
            raise ValueError("idle stable_for must be greater than zero")
        if self.timeout.nanoseconds == 0:
            raise ValueError("idle timeout must be greater than zero")
        if self.stable_for > self.timeout:
            raise ValueError("idle stable_for must not exceed timeout")


@dataclass(frozen=True, slots=True)
class ScreenPostcondition:
    predicate: JsonObject
    timeout: Duration
    kind: Literal["screen"] = "screen"

    def __post_init__(self) -> None:
        if self.timeout.nanoseconds == 0:
            raise ValueError("screen timeout must be greater than zero")


ActionPostcondition: TypeAlias = NonePostcondition | IdlePostcondition | ScreenPostcondition


@dataclass(frozen=True, slots=True)
class ActionOptions:
    request_id: str | None = None
    if_state: str | None = None
    caller_intent: str | None = None
    pace: Duration = Duration(0)
    verbose_atom_outcomes: bool = False
    postcondition: ActionPostcondition = NonePostcondition()
    frame: FrameRequest = FrameRequest()
    allow_incomplete: bool = False


@dataclass(frozen=True, slots=True)
class AtomOutcome:
    operation_id: str
    input_kind: InputKind
    bytes_written: int
    intent_event_seq: int | None
    result_event_seq: int | None


@dataclass(frozen=True, slots=True)
class InputResult:
    completed_atoms: int
    bytes_written: int
    first_event_seq: int | None
    last_event_seq: int | None
    pace: Duration
    atom_outcomes: tuple[AtomOutcome, ...] | None


@dataclass(frozen=True, slots=True)
class NoneCompletion:
    kind: Literal["none"] = "none"


@dataclass(frozen=True, slots=True)
class IdleCompletion:
    quality: Literal["stable", "child_completed", "output_closed", "deadline"]
    first_observed_at: Duration | None = None
    last_observed_at: Duration | None = None
    stable_for: Duration | None = None
    kind: Literal["idle"] = "idle"


@dataclass(frozen=True, slots=True)
class ScreenCompletion:
    quality: Literal["matched", "matched_unsettled", "child_completed", "output_closed", "deadline"]
    kind: Literal["screen"] = "screen"


ActionCompletion: TypeAlias = NoneCompletion | IdleCompletion | ScreenCompletion


def is_incomplete_completion(value: ActionCompletion) -> bool:
    if isinstance(value, NoneCompletion):
        return False
    return value.quality in ("child_completed", "output_closed", "deadline")


@dataclass(frozen=True, slots=True)
class ActionResult:
    request_id: str
    input: InputResult
    postcondition: ActionCompletion
    frame: ScreenFrame


@dataclass(frozen=True, slots=True)
class StableOptions:
    stable_for: Duration = Duration.milliseconds(100)
    timeout: Duration = Duration.seconds(10)
    frame: FrameRequest = FrameRequest()
    allow_incomplete: bool = False

    def __post_init__(self) -> None:
        if self.stable_for.nanoseconds == 0 or self.timeout.nanoseconds == 0:
            raise ValueError("stable_for and timeout must be greater than zero")
        if self.stable_for > self.timeout:
            raise ValueError("stable_for must not exceed timeout")


@dataclass(frozen=True, slots=True)
class ScreenWaitResult:
    matched: bool
    quality: Literal["matched", "matched_unsettled", "child_completed", "output_closed", "deadline"]
    first_match_at: Duration | None
    observed_at: Duration
    stable_for: Duration
    frame: ScreenFrame


@dataclass(frozen=True, slots=True)
class StableCapture:
    quality: StableQuality
    first_observed_at: Duration
    last_observed_at: Duration
    stable_for: Duration
    frame: ScreenFrame


__all__ = [
    "ActionCompletion",
    "ActionOptions",
    "ActionPostcondition",
    "ActionQuality",
    "ActionResult",
    "AtomOutcome",
    "IdleCompletion",
    "IdlePostcondition",
    "InputKind",
    "InputResult",
    "NoneCompletion",
    "NonePostcondition",
    "ScreenCompletion",
    "ScreenPostcondition",
    "ScreenWaitResult",
    "StableCapture",
    "StableOptions",
    "StableQuality",
    "is_incomplete_completion",
]
