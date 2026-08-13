"""Validated driver boundary and intentionally supported domain models."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, TypeAlias, TypeVar

import msgspec

from termtrace._generated.driver_request import DriverRequest as WireDriverRequest
from termtrace._generated.driver_response import (
    BundleManifest,
    ControlData,
    ControlErrorDetails,
    ControlStableSnapshotData,
    DiagnoseReport,
    DriverArtifactData,
    DriverAttachedData,
    DriverBundleData,
    DriverCompletionData,
    DriverControlData,
    DriverData,
    DriverDetachedData,
    DriverDiagnoseData,
    DriverDiffData,
    DriverDiscardedData,
    DriverErrorResult,
    DriverEventsData,
    DriverHelloData,
    DriverObservationData,
    DriverOutputWaitData,
    DriverScreenFrameData,
    DriverTimelineData,
    DriverVerificationData,
    EventProjection,
    InputSequenceFailureDetails,
    Observation,
    ObservationFrame,
    SemanticDiff,
    SessionCompletion,
    TimelineEntry,
    VerificationReport,
    WaitOutcome,
)
from termtrace._generated.driver_response import (
    DriverResponse as WireDriverResponse,
)
from termtrace.duration import Duration
from termtrace.errors import (
    ActionBusyError,
    InputSequenceBusyError,
    PartialInputError,
    ProtocolMismatchError,
    ResourceLimitError,
    StaleStateError,
    TermtraceOperationError,
    TermtraceProtocolError,
)
from termtrace.frames import FrameRequest, ScreenFrame

JsonScalar: TypeAlias = None | bool | int | float | str
JsonValue: TypeAlias = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject: TypeAlias = dict[str, JsonValue]

# Public domain names are intentionally independent from the driver envelopes.
Artifact: TypeAlias = DriverArtifactData
AttachedSession: TypeAlias = DriverAttachedData
BundleResult: TypeAlias = DriverBundleData
Completion: TypeAlias = SessionCompletion
ControlResult: TypeAlias = ControlData
StableSnapshot: TypeAlias = ControlStableSnapshotData
DiagnosticReport: TypeAlias = DiagnoseReport
EventPage: TypeAlias = DriverEventsData
TimelinePage: TypeAlias = DriverTimelineData
Frame: TypeAlias = ObservationFrame
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


ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)
_RESPONSE_DECODER = msgspec.json.Decoder(WireDriverResponse)


def encode_driver_request(method: JsonObject, request_id: str) -> bytes:
    """Parse and encode one client-authored request against the Rust contract."""
    try:
        request = msgspec.convert(
            {"version": 5, "request_id": request_id, "method": method},
            type=WireDriverRequest,
            strict=True,
        )
    except msgspec.ValidationError:
        raise TermtraceProtocolError("client request does not match the protocol schema") from None
    return msgspec.json.encode(request)


def decode_driver_response(
    encoded: bytes,
    expected_request_id: str,
    expected_type: type[ResponseDataT],
) -> ResponseDataT:
    """Decode, correlate, and narrow one complete driver response."""
    try:
        response = _RESPONSE_DECODER.decode(encoded)
    except msgspec.DecodeError:
        # Raw protocol data can contain terminal content, input, or environment data.
        raise TermtraceProtocolError("driver response does not match the protocol schema") from None
    if response.version != 5 or response.request_id != expected_request_id:
        raise TermtraceProtocolError("driver response version or request ID does not match")
    result = response.result
    if isinstance(result, DriverErrorResult):
        evidence = result.evidence if isinstance(result.evidence, str) else None
        details = result.details if isinstance(result.details, ControlErrorDetails) else None
        if result.code == "action_busy":
            raise ActionBusyError(
                result.message,
                retry_safety=result.retry_safety,
                uncertain=result.uncertain,
                evidence=evidence,
                request_id=expected_request_id,
                details=details,
            )
        if result.code == "input_sequence_busy":
            raise InputSequenceBusyError(
                result.message,
                retry_safety=result.retry_safety,
                uncertain=result.uncertain,
                evidence=evidence,
                request_id=expected_request_id,
                details=details,
            )
        if result.code in ("screen_frame_resource_limit", "resource_limit"):
            raise ResourceLimitError(
                result.message,
                retry_safety=result.retry_safety,
                uncertain=result.uncertain,
                evidence=evidence,
                request_id=expected_request_id,
                details=details,
            )
        if result.code in ("protocol_mismatch", "unsupported_protocol"):
            raise ProtocolMismatchError("driver rejected the protocol contract")
        if result.code == "stale_state" and details is not None:
            raise StaleStateError(
                result.message,
                expected_state_hash=None,
                current_state_hash=details.current_presented_state_hash,
                retry_safety=result.retry_safety,
                evidence=evidence,
                details=details,
                uncertain=result.uncertain,
                request_id=expected_request_id,
            )
        if (
            result.code == "input_sequence_incomplete"
            and details is not None
            and isinstance(details.input_sequence, InputSequenceFailureDetails)
        ):
            failure = details.input_sequence
            raise PartialInputError(
                result.message,
                completed_atoms=failure.atoms_applied,
                failed_atom_index=failure.failed_atom_index,
                operation_id=str(failure.failed_operation_id),
                bytes_written=failure.bytes_written,
                first_event_seq=failure.first_intent_event_seq,
                last_event_seq=failure.last_result_event_seq,
                resume_from_atom_index=failure.resume_from_atom_index,
                retry_safety=result.retry_safety,
                uncertain=result.uncertain,
                evidence=evidence,
                request_id=expected_request_id,
                details=details,
            )
        raise TermtraceOperationError(
            result.message,
            code=result.code,
            retry_safety=result.retry_safety,
            uncertain=result.uncertain,
            evidence=evidence,
            request_id=expected_request_id,
            details=details,
        )
    if not isinstance(result.data, expected_type):
        raise TermtraceProtocolError("driver returned an unexpected result kind")
    return result.data


__all__ = [
    "ActionCompletion",
    "ActionOptions",
    "ActionPostcondition",
    "ActionQuality",
    "ActionResult",
    "Artifact",
    "AtomOutcome",
    "AttachedSession",
    "BundleManifest",
    "BundleResult",
    "Completion",
    "ControlErrorDetails",
    "ControlResult",
    "ControlStableSnapshotData",
    "DiagnosticReport",
    "DriverArtifactData",
    "DriverAttachedData",
    "DriverBundleData",
    "DriverCompletionData",
    "DriverControlData",
    "DriverData",
    "DriverDetachedData",
    "DriverDiagnoseData",
    "DriverDiffData",
    "DriverDiscardedData",
    "DriverEventsData",
    "DriverHelloData",
    "DriverObservationData",
    "DriverOutputWaitData",
    "DriverScreenFrameData",
    "DriverTimelineData",
    "DriverVerificationData",
    "EventPage",
    "EventProjection",
    "Frame",
    "IdleCompletion",
    "IdlePostcondition",
    "InputResult",
    "InputKind",
    "InputSequenceFailureDetails",
    "JsonObject",
    "JsonValue",
    "NoneCompletion",
    "NonePostcondition",
    "Observation",
    "ScreenCompletion",
    "ScreenPostcondition",
    "ScreenWaitResult",
    "SemanticDiff",
    "SessionCompletion",
    "StableCapture",
    "StableOptions",
    "StableQuality",
    "StableSnapshot",
    "TimelineEntry",
    "TimelinePage",
    "VerificationReport",
    "WaitOutcome",
    "decode_driver_response",
    "encode_driver_request",
    "is_incomplete_completion",
]
