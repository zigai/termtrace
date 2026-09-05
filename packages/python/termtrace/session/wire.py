"""Action request serialization and response parsing."""

from __future__ import annotations

from collections.abc import Mapping

import msgspec

from termtrace.duration import Duration
from termtrace.errors import ProtocolMismatchError
from termtrace.screen.wire import normalize_frame
from termtrace.session.actions import (
    ActionResult,
    AtomOutcome,
    IdleCompletion,
    IdlePostcondition,
    InputResult,
    NoneCompletion,
    NonePostcondition,
    ScreenCompletion,
    ScreenPostcondition,
    ScreenWaitResult,
    StableCapture,
)
from termtrace.transport.generated import driver_request as request_wire
from termtrace.transport.generated import driver_response as response_wire
from termtrace.transport.protocol import JsonObject, optional_wire, parse_wire, wire_json_object


def encode_postcondition(
    value: NonePostcondition | IdlePostcondition | ScreenPostcondition,
) -> JsonObject:
    if isinstance(value, NonePostcondition):
        wire_value: request_wire.ActionPostcondition = request_wire.ActionPostcondition1()
    elif isinstance(value, IdlePostcondition):
        wire_value = request_wire.ActionPostcondition2(
            stable_for_ns=value.stable_for.nanoseconds,
            timeout_ns=value.timeout.nanoseconds,
        )
    else:
        wire_value = msgspec.convert(
            {
                "condition": "screen",
                "predicate": value.predicate,
                "timeout_ns": value.timeout.nanoseconds,
            },
            type=request_wire.ActionPostcondition,
        )
    return wire_json_object(wire_value)


def normalize_action(
    value: response_wire.ControlData | Mapping[str, object],
    request_id: str,
) -> ActionResult:
    wire_value = parse_wire(value, response_wire.ControlActionAppliedData, "action response")
    input_value = wire_value.input
    if input_value.sequence_id != request_id:
        raise ProtocolMismatchError("action request ID does not match its canonical sequence ID")

    atom_outcomes: tuple[AtomOutcome, ...] | None = None
    if not isinstance(input_value.atoms, msgspec.UnsetType):
        atom_outcomes = tuple(_atom_outcome(item) for item in input_value.atoms)

    input_result = InputResult(
        completed_atoms=input_value.atom_count,
        bytes_written=input_value.bytes_written,
        first_event_seq=input_value.first_intent_event_seq,
        last_event_seq=input_value.last_result_event_seq,
        pace=Duration(input_value.pace_ns),
        atom_outcomes=atom_outcomes,
    )
    postcondition = wire_value.postcondition
    if isinstance(postcondition, response_wire.ActionPostconditionResult1):
        completion: NoneCompletion | IdleCompletion | ScreenCompletion = NoneCompletion()
    elif isinstance(postcondition, response_wire.ActionPostconditionResult2):
        completion = IdleCompletion(postcondition.quality.value)
    else:
        completion = ScreenCompletion(postcondition.quality.value)
    return ActionResult(
        request_id,
        input_result,
        completion,
        normalize_frame(postcondition.frame),
    )


def normalize_stable_capture(
    value: response_wire.ControlData | Mapping[str, object],
) -> StableCapture:
    wire_value = parse_wire(value, response_wire.ControlStableSnapshotData, "stable capture")
    return StableCapture(
        quality=wire_value.quality.value,
        first_observed_at=Duration(wire_value.first_observed_monotonic_ns),
        last_observed_at=Duration(wire_value.observed_monotonic_ns),
        stable_for=Duration(wire_value.stable_for_ns),
        frame=normalize_frame(wire_value.frame),
    )


def normalize_screen_wait(
    value: response_wire.ControlData | Mapping[str, object],
) -> ScreenWaitResult:
    wire_value = parse_wire(value, response_wire.ControlScreenWaitData, "screen wait")
    first_match = optional_wire(wire_value.first_match_monotonic_ns)
    return ScreenWaitResult(
        matched=wire_value.matched,
        quality=wire_value.quality.value,
        first_match_at=None if first_match is None else Duration(first_match),
        observed_at=Duration(wire_value.observed_monotonic_ns),
        stable_for=Duration(wire_value.stable_for_ns),
        frame=normalize_frame(wire_value.frame),
    )


def _atom_outcome(value: response_wire.InputSequenceAtomOutcome) -> AtomOutcome:
    return AtomOutcome(
        value.operation_id,
        value.input_kind.value,
        value.bytes_written,
        optional_wire(value.intent_event_seq),
        optional_wire(value.result_event_seq),
    )


__all__ = [
    "encode_postcondition",
    "normalize_action",
    "normalize_screen_wait",
    "normalize_stable_capture",
]
