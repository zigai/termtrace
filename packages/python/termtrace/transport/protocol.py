"""Validated driver protocol boundary and generated response projections."""

from __future__ import annotations

from collections.abc import Mapping
from typing import TypeAlias, TypeVar

import msgspec

from termtrace.errors import (
    ActionBusyError,
    InputSequenceBusyError,
    PartialInputError,
    ProtocolMismatchError,
    ResourceLimitError,
    StaleStateError,
    TermtraceOperationError,
)
from termtrace.transport.generated.driver_request import DriverRequest as WireDriverRequest
from termtrace.transport.generated.driver_response import (
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
from termtrace.transport.generated.driver_response import (
    DriverResponse as WireDriverResponse,
)

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

ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)
WireT = TypeVar("WireT", bound=msgspec.Struct)
OptionalT = TypeVar("OptionalT")
_RESPONSE_DECODER = msgspec.json.Decoder(WireDriverResponse)


def parse_wire(
    value: msgspec.Struct | Mapping[str, object],
    wire_type: type[WireT],
    label: str,
) -> WireT:
    if isinstance(value, wire_type):
        return value
    data = msgspec.to_builtins(value) if isinstance(value, msgspec.Struct) else value
    try:
        return msgspec.convert(data, type=wire_type, strict=True)
    except msgspec.ValidationError:
        raise ProtocolMismatchError(f"{label} does not match the protocol schema") from None


def optional_wire(value: OptionalT | None | msgspec.UnsetType) -> OptionalT | None:
    return None if isinstance(value, msgspec.UnsetType) else value


def wire_json_object(value: msgspec.Struct) -> JsonObject:
    data = msgspec.to_builtins(value)
    if not isinstance(data, dict) or not all(isinstance(key, str) for key in data):
        raise AssertionError("msgspec struct did not serialize to an object")
    return data


def encode_driver_request(method: JsonObject, request_id: str) -> bytes:
    """Parse and encode one client-authored request against the Rust contract."""
    try:
        request = msgspec.convert(
            {"version": 5, "request_id": request_id, "method": method},
            type=WireDriverRequest,
            strict=True,
        )
    except msgspec.ValidationError:
        raise ProtocolMismatchError("client request does not match the protocol schema") from None
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
        raise ProtocolMismatchError("driver response does not match the protocol schema") from None
    if response.version != 5 or response.request_id != expected_request_id:
        raise ProtocolMismatchError("driver response version or request ID does not match")
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
        raise ProtocolMismatchError("driver returned an unexpected result kind")
    return result.data


__all__ = [
    "Artifact",
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
    "InputSequenceFailureDetails",
    "JsonObject",
    "JsonValue",
    "Observation",
    "SemanticDiff",
    "SessionCompletion",
    "StableSnapshot",
    "TimelineEntry",
    "TimelinePage",
    "VerificationReport",
    "WaitOutcome",
    "decode_driver_response",
    "encode_driver_request",
    "optional_wire",
    "parse_wire",
    "wire_json_object",
]
