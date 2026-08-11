"""Validated driver boundary and intentionally supported domain models."""

from __future__ import annotations

from typing import TypeAlias, TypeVar

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
from termtrace.errors import TermtraceOperationError, TermtraceProtocolError

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
_RESPONSE_DECODER = msgspec.json.Decoder(WireDriverResponse)
_REQUEST_DECODER = msgspec.json.Decoder(WireDriverRequest)


def encode_driver_request(method: JsonObject, request_id: str) -> bytes:
    """Encode and validate one client-authored request against the Rust contract."""

    encoded = msgspec.json.encode(
        {"version": 4, "request_id": request_id, "method": method}
    )
    try:
        _REQUEST_DECODER.decode(encoded)
    except msgspec.DecodeError:
        raise TermtraceProtocolError(
            "client request does not match the protocol schema"
        ) from None
    return encoded


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
        raise TermtraceProtocolError(
            "driver response does not match the protocol schema"
        ) from None
    if response.version != 4 or response.request_id != expected_request_id:
        raise TermtraceProtocolError(
            "driver response version or request ID does not match"
        )
    result = response.result
    if isinstance(result, DriverErrorResult):
        evidence = result.evidence if isinstance(result.evidence, str) else None
        raise TermtraceOperationError(
            result.message,
            code=result.code,
            retry_safety=result.retry_safety,
            uncertain=result.uncertain,
            evidence=evidence,
            details=(
                result.details
                if isinstance(result.details, ControlErrorDetails)
                else None
            ),
        )
    if not isinstance(result.data, expected_type):
        raise TermtraceProtocolError("driver returned an unexpected result kind")
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
]
