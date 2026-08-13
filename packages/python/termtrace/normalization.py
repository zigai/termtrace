"""Typed wire-to-domain parsing and serialization boundaries."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Annotated, TypeVar

import msgspec

from termtrace._generated import driver_request as request_wire
from termtrace._generated import driver_response as response_wire
from termtrace.duration import Duration
from termtrace.errors import ProtocolMismatchError
from termtrace.frames import (
    ActorBoundaries,
    Appearance,
    CanonicalBoundary,
    CellStyle,
    Color,
    Colors,
    Cursor,
    FrameDetail,
    FrameRequest,
    FrameSource,
    Geometry,
    Modes,
    Region,
    Rgb,
    ScreenCell,
    ScreenFrame,
    ScreenRow,
    ScreenSpan,
    SpanCell,
    StatePoint,
    TerminalMode,
)
from termtrace.models import (
    ActionResult,
    AtomOutcome,
    IdleCompletion,
    IdlePostcondition,
    InputResult,
    JsonObject,
    NoneCompletion,
    NonePostcondition,
    ScreenCompletion,
    ScreenPostcondition,
    ScreenWaitResult,
    StableCapture,
)

WireT = TypeVar("WireT", bound=msgspec.Struct)
OptionalT = TypeVar("OptionalT")


class _StableCapturePayload(
    msgspec.Struct,
    tag_field="kind",
    tag="stable_snapshot",
):
    quality: response_wire.StableCaptureQuality
    first_observed_monotonic_ns: Annotated[int, msgspec.Meta(ge=0)]
    observed_monotonic_ns: Annotated[int, msgspec.Meta(ge=0)]
    stable_for_ns: Annotated[int, msgspec.Meta(ge=0)]
    frame: response_wire.ScreenFrame


class _ScreenWaitPayload(
    msgspec.Struct,
    tag_field="kind",
    tag="screen_wait",
):
    matched: bool
    quality: response_wire.ScreenWaitQuality
    observed_monotonic_ns: Annotated[int, msgspec.Meta(ge=0)]
    stable_for_ns: Annotated[int, msgspec.Meta(ge=0)]
    frame: response_wire.ScreenFrame
    first_match_monotonic_ns: Annotated[int, msgspec.Meta(ge=0)] | None | msgspec.UnsetType = (
        msgspec.UNSET
    )


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
    return _json_object(wire_value)


def encode_frame_request(value: FrameRequest) -> JsonObject:
    region: request_wire.ScreenRegion | None = None
    if value.region is not None:
        region = request_wire.ScreenRegion(
            top=value.region.top,
            left=value.region.left,
            right=value.region.right,
            bottom=value.region.bottom,
        )
    wire_value = request_wire.ScreenProjectionRequest(
        detail=request_wire.FrameDetail(value.detail),
        region=region,
    )
    return _json_object(wire_value)


def normalize_frame(
    value: response_wire.ScreenFrame | Mapping[str, object],
) -> ScreenFrame:
    wire_value = _parse(value, response_wire.ScreenFrame, "screen frame")
    detail, rows = _rows(wire_value.data)
    source = _source(wire_value.source)
    return ScreenFrame(
        version=wire_value.version,
        state_hash=wire_value.state_hash,
        projection_hash=wire_value.frame_hash,
        presented=wire_value.presented,
        capture_quality=wire_value.capture_quality.value,
        source=source,
        boundaries=source.boundaries,
        geometry=_geometry(wire_value.geometry),
        requested_region=_region(wire_value.requested_region),
        returned_region=_region(wire_value.returned_region),
        cursor=_cursor(wire_value.cursor),
        modes=_modes(wire_value.modes),
        colors=_colors(wire_value.colors),
        rows=rows,
        detail=detail,
    )


def normalize_action(
    value: response_wire.ControlData | Mapping[str, object],
    request_id: str,
) -> ActionResult:
    wire_value = _parse(value, response_wire.ControlData4, "action response")
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
    wire_value = (
        value
        if isinstance(value, response_wire.ControlData10)
        else _parse(value, _StableCapturePayload, "stable capture")
    )
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
    wire_value = (
        value
        if isinstance(value, response_wire.ControlData11)
        else _parse(value, _ScreenWaitPayload, "screen wait")
    )
    first_match = _optional(wire_value.first_match_monotonic_ns)
    return ScreenWaitResult(
        matched=wire_value.matched,
        quality=wire_value.quality.value,
        first_match_at=None if first_match is None else Duration(first_match),
        observed_at=Duration(wire_value.observed_monotonic_ns),
        stable_for=Duration(wire_value.stable_for_ns),
        frame=normalize_frame(wire_value.frame),
    )


def _parse(
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


def _optional(value: OptionalT | None | msgspec.UnsetType) -> OptionalT | None:
    if value is None or isinstance(value, msgspec.UnsetType):
        return None
    return value


def _json_object(value: msgspec.Struct) -> JsonObject:
    data = msgspec.to_builtins(value)
    if not isinstance(data, dict) or not all(isinstance(key, str) for key in data):
        raise AssertionError("msgspec struct did not serialize to an object")
    return data


def _rows(
    value: response_wire.ScreenFrameData,
) -> tuple[FrameDetail, tuple[ScreenRow, ...]]:
    if isinstance(value, response_wire.ScreenFrameData1):
        return "text", tuple(_row(item) for item in value.rows)
    if isinstance(value, response_wire.ScreenFrameData2):
        return "spans", tuple(_row(item) for item in value.rows)
    return "cells", tuple(_row(item) for item in value.rows)


def _row(
    value: response_wire.ScreenTextRow | response_wire.ScreenSpanRow | response_wire.ScreenCellRow,
) -> ScreenRow:
    spans: tuple[ScreenSpan, ...] = ()
    cells: tuple[ScreenCell, ...] = ()
    if isinstance(value, response_wire.ScreenSpanRow):
        spans = tuple(_span(item) for item in value.spans)
    elif isinstance(value, response_wire.ScreenCellRow):
        cells = tuple(_cell(item) for item in value.cells)
    return ScreenRow(
        value.row,
        value.soft_wrapped,
        value.wrap_continuation,
        value.text,
        spans,
        cells,
    )


def _span(value: response_wire.ScreenSpan) -> ScreenSpan:
    cells = tuple(_span_cell(item) for item in value.cells)
    if value.cell_count != len(cells):
        raise ProtocolMismatchError("screen span cell count is inconsistent")
    return ScreenSpan(
        value.start_column,
        value.end_column,
        value.text,
        _appearance(value.appearance),
        cells,
    )


def _span_cell(value: response_wire.SpanCell) -> SpanCell:
    return SpanCell(value.grapheme, value.wide_kind.value)


def _cell(value: response_wire.ScreenCell) -> ScreenCell:
    return ScreenCell(
        value.row,
        value.column,
        value.grapheme,
        value.wide_kind.value,
        _appearance(value.appearance),
    )


def _appearance(value: response_wire.ScreenAppearance) -> Appearance:
    foreground = _optional(value.resolved_foreground)
    background = _optional(value.resolved_background)
    return Appearance(
        _style(value.style),
        None if foreground is None else _rgb(foreground),
        None if background is None else _rgb(background),
        value.selected,
    )


def _style(value: response_wire.CellStyle) -> CellStyle:
    return CellStyle(
        _color(value.foreground),
        _color(value.background),
        _color(value.underline_color),
        value.bold,
        value.faint,
        value.italic,
        value.underline,
        value.blink,
        value.inverse,
        value.invisible,
        value.strikethrough,
        value.overline,
    )


def _color(value: response_wire.ColorSpec) -> Color:
    if isinstance(value, response_wire.ColorSpec1):
        return Color("none")
    if isinstance(value, response_wire.ColorSpec2):
        return Color("palette", index=value.value)
    return Color("rgb", value=_rgb(value.value))


def _geometry(value: response_wire.ScreenGeometry) -> Geometry:
    return Geometry(value.cols, value.rows)


def _region(value: response_wire.ScreenRegion) -> Region:
    try:
        return Region(
            value.top,
            value.left,
            value.right - value.left,
            value.bottom - value.top,
        )
    except ValueError as e:
        raise ProtocolMismatchError("screen frame contains an invalid region") from e


def _rgb(value: response_wire.Rgb) -> Rgb:
    return Rgb(value.r, value.g, value.b)


def _cursor(value: response_wire.CursorState) -> Cursor:
    return Cursor(
        _optional(value.row),
        _optional(value.column),
        value.visible,
        value.blinking,
        value.style.value,
        value.in_viewport,
        _optional(value.wide_tail),
    )


def _modes(value: response_wire.ModeState) -> Modes:
    return Modes(
        value.alternate_screen,
        value.bracketed_paste,
        value.synchronized_output,
        value.wraparound,
        tuple(_mode(item) for item in value.values),
    )


def _mode(value: response_wire.TerminalMode) -> TerminalMode:
    return TerminalMode(value.ansi, value.enabled, value.number)


def _colors(value: response_wire.ColorState) -> Colors:
    cursor = _optional(value.cursor)
    return Colors(
        _rgb(value.foreground),
        _rgb(value.background),
        None if cursor is None else _rgb(cursor),
        tuple(_rgb(item) for item in value.palette),
    )


def _boundary(value: response_wire.CanonicalBoundary) -> CanonicalBoundary:
    return CanonicalBoundary(value.event_hash, value.next_event_seq, value.output_bytes)


def _boundaries(value: response_wire.ActorBoundaries) -> ActorBoundaries:
    return ActorBoundaries(_boundary(value.durable), _boundary(value.observed))


def _state_point(value: response_wire.StatePoint) -> StatePoint:
    if isinstance(value, response_wire.StatePoint1):
        return StatePoint("latest_presented")
    if isinstance(value, response_wire.StatePoint2):
        return StatePoint("state_id", state_id=value.state_id)
    if isinstance(value, response_wire.StatePoint3):
        return StatePoint("state_hash", state_hash=value.state_hash)
    if isinstance(value, response_wire.StatePoint4):
        return StatePoint("event_seq", event_seq=value.event_seq)
    if isinstance(value, response_wire.StatePoint5):
        return StatePoint("monotonic_ns", monotonic_ns=value.monotonic_ns)
    return StatePoint("marker", label=value.label)


def _source(value: response_wire.ScreenFrameSource) -> FrameSource:
    if isinstance(value, response_wire.ScreenFrameSource1):
        return FrameSource(
            "actor",
            session_id=value.session_id,
            boundaries=_boundaries(value.boundaries),
            observed_monotonic_ns=value.observed_monotonic_ns,
        )
    return FrameSource(
        "resolved",
        session_id=_optional(value.session_id),
        source_kind=value.source_kind.value,
        source_completed=value.source_completed,
        state_point=_state_point(value.state_point),
        state_id=_optional(value.state_id),
        event_seq=_optional(value.event_seq),
        monotonic_ns=_optional(value.monotonic_ns),
        boundary=_boundary(value.boundary),
        integrity=value.integrity.value,
        resolution=value.resolution.value,
    )


def _atom_outcome(value: response_wire.InputSequenceAtomOutcome) -> AtomOutcome:
    return AtomOutcome(
        value.operation_id,
        value.input_kind.value,
        value.bytes_written,
        _optional(value.intent_event_seq),
        _optional(value.result_event_seq),
    )


__all__ = [
    "encode_frame_request",
    "encode_postcondition",
    "normalize_action",
    "normalize_frame",
    "normalize_screen_wait",
    "normalize_stable_capture",
]
