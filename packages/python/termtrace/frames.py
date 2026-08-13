"""Immutable rich terminal frame domain values."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, TypeAlias

from termtrace.errors import InvalidRegionError, ProtocolMismatchError

FrameDetail: TypeAlias = Literal["text", "spans", "cells"]

if TYPE_CHECKING:
    from termtrace.locators import LocatorSpec, Query
WideKind: TypeAlias = Literal["narrow", "wide", "spacer_tail", "spacer_head"]
CaptureQuality: TypeAlias = Literal["live", "stable", "completed", "incomplete"]


@dataclass(frozen=True, slots=True)
class Region:
    top: int
    left: int
    width: int
    height: int

    def __post_init__(self) -> None:
        values = (self.top, self.left, self.width, self.height)
        if any(isinstance(value, bool) or not isinstance(value, int) for value in values):
            raise InvalidRegionError("region coordinates must be integers")
        if self.top < 0 or self.left < 0 or self.width <= 0 or self.height <= 0:
            raise InvalidRegionError("region origin must be nonnegative and dimensions nonzero")
        if self.right > 65_535 or self.bottom > 65_535:
            raise InvalidRegionError("region edges exceed terminal coordinate bounds")

    @property
    def right(self) -> int:
        return self.left + self.width

    @property
    def bottom(self) -> int:
        return self.top + self.height

    def contains(self, row: int, column: int) -> bool:
        return self.top <= row < self.bottom and self.left <= column < self.right

    def to_wire(self) -> dict[str, int]:
        return {
            "top": self.top,
            "left": self.left,
            "right": self.right,
            "bottom": self.bottom,
        }


@dataclass(frozen=True, slots=True)
class FrameRequest:
    detail: FrameDetail = "spans"
    region: Region | None = None

    def __post_init__(self) -> None:
        if self.detail not in ("text", "spans", "cells"):
            raise ValueError(f"unsupported frame detail {self.detail!r}")
        if self.detail == "cells" and self.region is None:
            raise InvalidRegionError("exact cell detail requires an explicit region")

    def to_wire(self) -> dict[str, str | dict[str, int] | None]:
        return {
            "detail": self.detail,
            "region": self.region.to_wire() if self.region is not None else None,
        }


@dataclass(frozen=True, slots=True)
class Geometry:
    cols: int
    rows: int


@dataclass(frozen=True, slots=True)
class Rgb:
    r: int
    g: int
    b: int

    def __post_init__(self) -> None:
        if any(not 0 <= value <= 255 for value in (self.r, self.g, self.b)):
            raise ProtocolMismatchError("RGB channel is outside 0 through 255")


@dataclass(frozen=True, slots=True)
class Color:
    kind: Literal["none", "palette", "rgb"]
    index: int | None = None
    value: Rgb | None = None


@dataclass(frozen=True, slots=True)
class CellStyle:
    foreground: Color
    background: Color
    underline_color: Color
    bold: bool
    faint: bool
    italic: bool
    underline: int
    blink: bool
    inverse: bool
    invisible: bool
    strikethrough: bool
    overline: bool


@dataclass(frozen=True, slots=True)
class Appearance:
    style: CellStyle
    resolved_foreground: Rgb | None
    resolved_background: Rgb | None
    selected: bool


@dataclass(frozen=True, slots=True)
class SpanCell:
    grapheme: str
    wide_kind: WideKind


@dataclass(frozen=True, slots=True)
class ScreenSpan:
    start_column: int
    end_column: int
    text: str
    appearance: Appearance
    cells: tuple[SpanCell, ...]

    def __post_init__(self) -> None:
        if (
            self.end_column <= self.start_column
            or len(self.cells) != self.end_column - self.start_column
        ):
            raise ProtocolMismatchError("screen span has inconsistent column coverage")

    @property
    def cell_count(self) -> int:
        return len(self.cells)


@dataclass(frozen=True, slots=True)
class ScreenCell:
    row: int
    column: int
    grapheme: str
    wide_kind: WideKind
    appearance: Appearance


@dataclass(frozen=True, slots=True)
class ScreenRow:
    row: int
    soft_wrapped: bool
    wrap_continuation: bool
    text: str
    spans: tuple[ScreenSpan, ...] = ()
    cells: tuple[ScreenCell, ...] = ()

    def expanded_cells(self) -> tuple[ScreenCell, ...]:
        if self.cells:
            return self.cells
        expanded: list[ScreenCell] = []
        for span in self.spans:
            for offset, cell in enumerate(span.cells):
                expanded.append(
                    ScreenCell(
                        self.row,
                        span.start_column + offset,
                        cell.grapheme,
                        cell.wide_kind,
                        span.appearance,
                    )
                )
        return tuple(expanded)


@dataclass(frozen=True, slots=True)
class Cursor:
    row: int | None
    column: int | None
    visible: bool
    blinking: bool
    style: str
    in_viewport: bool
    wide_tail: bool | None = None


@dataclass(frozen=True, slots=True)
class TerminalMode:
    ansi: bool
    enabled: bool
    number: int


@dataclass(frozen=True, slots=True)
class Modes:
    alternate_screen: bool
    bracketed_paste: bool
    synchronized_output: bool
    wraparound: bool
    values: tuple[TerminalMode, ...]


@dataclass(frozen=True, slots=True)
class Colors:
    foreground: Rgb
    background: Rgb
    cursor: Rgb | None
    palette: tuple[Rgb, ...]


@dataclass(frozen=True, slots=True)
class CanonicalBoundary:
    event_hash: str
    next_event_seq: int
    output_bytes: int


@dataclass(frozen=True, slots=True)
class ActorBoundaries:
    durable: CanonicalBoundary
    observed: CanonicalBoundary


@dataclass(frozen=True, slots=True)
class StatePoint:
    selector: Literal[
        "latest_presented",
        "state_id",
        "state_hash",
        "event_seq",
        "monotonic_ns",
        "marker",
    ]
    state_id: int | None = None
    state_hash: str | None = None
    event_seq: int | None = None
    monotonic_ns: int | None = None
    label: str | None = None


@dataclass(frozen=True, slots=True)
class FrameSource:
    kind: Literal["actor", "resolved"]
    session_id: str | None = None
    boundaries: ActorBoundaries | None = None
    observed_monotonic_ns: int | None = None
    source_kind: str | None = None
    source_completed: bool | None = None
    state_point: StatePoint | None = None
    state_id: int | None = None
    event_seq: int | None = None
    monotonic_ns: int | None = None
    boundary: CanonicalBoundary | None = None
    integrity: str | None = None
    resolution: str | None = None


@dataclass(frozen=True, slots=True)
class ScreenFrame:
    version: int
    state_hash: str
    projection_hash: str
    presented: bool
    capture_quality: str
    source: FrameSource
    boundaries: ActorBoundaries | None
    geometry: Geometry
    requested_region: Region
    returned_region: Region
    cursor: Cursor
    modes: Modes
    colors: Colors
    rows: tuple[ScreenRow, ...]
    detail: FrameDetail

    def __post_init__(self) -> None:
        if self.version != 1:
            raise ProtocolMismatchError(f"unsupported screen frame version {self.version}")
        if self.returned_region != self.requested_region:
            raise ProtocolMismatchError("screen frame region was unexpectedly clipped")
        if (
            self.requested_region.right > self.geometry.cols
            or self.requested_region.bottom > self.geometry.rows
        ):
            raise ProtocolMismatchError("screen frame region exceeds its geometry")
        if len(self.rows) != self.returned_region.height:
            raise ProtocolMismatchError("screen frame row count does not cover its region")
        for row_offset, row in enumerate(self.rows):
            expected_row = self.returned_region.top + row_offset
            if row.row != expected_row:
                raise ProtocolMismatchError("screen rows are not contiguous")
            if self.detail == "cells":
                if len(row.cells) != self.returned_region.width:
                    raise ProtocolMismatchError("screen cell coverage is inconsistent")
                for column_offset, cell in enumerate(row.cells):
                    if (
                        cell.row != expected_row
                        or cell.column != self.returned_region.left + column_offset
                    ):
                        raise ProtocolMismatchError("screen cell coverage is inconsistent")
            if self.detail == "spans":
                expected_column = self.returned_region.left
                for span in row.spans:
                    if span.start_column != expected_column:
                        raise ProtocolMismatchError("screen spans are not contiguous")
                    expected_column = span.end_column
                if expected_column != self.returned_region.right:
                    raise ProtocolMismatchError("screen spans do not cover the returned region")

    @property
    def frame_hash(self) -> str:
        return self.projection_hash

    @property
    def text(self) -> str:
        return "\n".join(row.text for row in self.rows)

    @property
    def viewport_text(self) -> str:
        return self.text

    @property
    def cells(self) -> tuple[ScreenCell, ...]:
        return tuple(cell for row in self.rows for cell in row.expanded_cells())

    def query(self, spec: LocatorSpec) -> Query:
        from termtrace.locators import LocatorSpec, Query

        if not isinstance(spec, LocatorSpec):
            raise TypeError("query requires a LocatorSpec")
        return Query(self, spec)


__all__ = [
    "ActorBoundaries",
    "Appearance",
    "CanonicalBoundary",
    "CaptureQuality",
    "CellStyle",
    "Color",
    "Colors",
    "Cursor",
    "FrameDetail",
    "FrameRequest",
    "FrameSource",
    "Geometry",
    "Modes",
    "Region",
    "Rgb",
    "ScreenCell",
    "ScreenFrame",
    "ScreenRow",
    "ScreenSpan",
    "SpanCell",
    "StatePoint",
    "TerminalMode",
    "WideKind",
]
