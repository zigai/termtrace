"""Pure deterministic rich frame snapshot normalization."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, TypeAlias, TypeGuard

from termtrace.errors import ResourceLimitError
from termtrace.screen.frame import (
    Appearance,
    Color,
    Colors,
    Region,
    Rgb,
    ScreenCell,
    ScreenFrame,
    ScreenRow,
    ScreenSpan,
    SpanCell,
)

SnapshotScalar: TypeAlias = None | bool | int | str
SnapshotValue: TypeAlias = (
    SnapshotScalar | tuple["SnapshotValue", ...] | tuple[tuple[str, "SnapshotValue"], ...]
)
SnapshotProfile: TypeAlias = Literal["text", "spans", "cells"]


@dataclass(frozen=True, slots=True)
class FrameSnapshot:
    profile: SnapshotProfile
    fields: tuple[tuple[str, SnapshotValue], ...]

    def to_dict(self) -> dict[str, object]:
        return {key: _thaw(value) for key, value in self.fields}


def normalize_frame(
    frame: ScreenFrame,
    profile: SnapshotProfile,
    *,
    include_cursor: bool = True,
    include_modes: bool = True,
    maximum_cells: int = 16_384,
) -> FrameSnapshot:
    """Normalize an already captured exact frame without issuing I/O."""
    if profile not in ("text", "spans", "cells"):
        raise ValueError(f"unknown snapshot profile {profile!r}")
    if profile == "cells":
        cells = sum(
            len(row.cells) if row.cells else sum(len(span.cells) for span in row.spans)
            for row in frame.rows
        )
        if cells > maximum_cells:
            raise ResourceLimitError(
                f"cell snapshot requested {cells} cells; maximum is {maximum_cells}"
            )
    values: list[tuple[str, SnapshotValue]] = [
        ("version", frame.version),
        ("profile", profile),
        ("capture_quality", frame.capture_quality),
        ("presented", frame.presented),
        ("geometry", (("cols", frame.geometry.cols), ("rows", frame.geometry.rows))),
        ("requested_region", _region(frame.requested_region)),
        ("returned_region", _region(frame.returned_region)),
        ("colors", _colors(frame.colors)),
        ("rows", tuple(_row(row, profile) for row in frame.rows)),
    ]
    if include_cursor:
        values.append(
            (
                "cursor",
                (
                    ("row", frame.cursor.row),
                    ("column", frame.cursor.column),
                    ("visible", frame.cursor.visible),
                    ("blinking", frame.cursor.blinking),
                    ("style", frame.cursor.style),
                    ("in_viewport", frame.cursor.in_viewport),
                    ("wide_tail", frame.cursor.wide_tail),
                ),
            )
        )
    if include_modes:
        values.append(
            (
                "modes",
                (
                    ("alternate_screen", frame.modes.alternate_screen),
                    ("bracketed_paste", frame.modes.bracketed_paste),
                    ("synchronized_output", frame.modes.synchronized_output),
                    ("wraparound", frame.modes.wraparound),
                    (
                        "values",
                        tuple(
                            (
                                ("ansi", mode.ansi),
                                ("enabled", mode.enabled),
                                ("number", mode.number),
                            )
                            for mode in frame.modes.values
                        ),
                    ),
                ),
            )
        )
    return FrameSnapshot(profile, tuple(values))


def _region(value: Region) -> SnapshotValue:
    return (
        ("top", value.top),
        ("left", value.left),
        ("width", value.width),
        ("height", value.height),
    )


def _colors(value: Colors) -> SnapshotValue:
    return (
        ("foreground", _rgb(value.foreground)),
        ("background", _rgb(value.background)),
        ("cursor", _rgb(value.cursor)),
        ("palette", tuple(_rgb(color) for color in value.palette)),
    )


def _row(row: ScreenRow, profile: SnapshotProfile) -> SnapshotValue:
    fields: list[tuple[str, SnapshotValue]] = [
        ("row", row.row),
        ("soft_wrapped", row.soft_wrapped),
        ("wrap_continuation", row.wrap_continuation),
        ("text", row.text),
    ]
    if profile in ("spans", "cells"):
        fields.append(("spans", tuple(_span(span) for span in _spans(row))))
    if profile == "cells":
        fields.append(("cells", tuple(_cell(cell) for cell in row.expanded_cells())))
    return tuple(fields)


def _spans(row: ScreenRow) -> tuple[ScreenSpan, ...]:
    if row.spans:
        return row.spans
    cells = row.expanded_cells()
    if not cells:
        return ()
    spans: list[ScreenSpan] = []
    start = 0
    for index in range(1, len(cells) + 1):
        if (
            index < len(cells)
            and cells[index].appearance == cells[start].appearance
            and cells[index].column == cells[index - 1].column + 1
        ):
            continue
        group = cells[start:index]
        text = "".join(
            cell.grapheme or (" " if cell.wide_kind == "narrow" else "") for cell in group
        )
        spans.append(
            ScreenSpan(
                group[0].column,
                group[-1].column + 1,
                text,
                group[0].appearance,
                tuple(SpanCell(cell.grapheme, cell.wide_kind) for cell in group),
            )
        )
        start = index
    return tuple(spans)


def _span(span: ScreenSpan) -> SnapshotValue:
    return (
        ("start_column", span.start_column),
        ("end_column", span.end_column),
        ("text", span.text),
        ("appearance", _appearance(span.appearance)),
        (
            "cells",
            tuple(
                (("grapheme", cell.grapheme), ("wide_kind", cell.wide_kind)) for cell in span.cells
            ),
        ),
    )


def _cell(cell: ScreenCell) -> SnapshotValue:
    return (
        ("row", cell.row),
        ("column", cell.column),
        ("grapheme", cell.grapheme),
        ("wide_kind", cell.wide_kind),
        ("appearance", _appearance(cell.appearance)),
    )


def _appearance(value: Appearance) -> SnapshotValue:
    style = value.style
    return (
        (
            "style",
            (
                ("foreground", _color(style.foreground)),
                ("background", _color(style.background)),
                ("underline_color", _color(style.underline_color)),
                ("bold", style.bold),
                ("faint", style.faint),
                ("italic", style.italic),
                ("underline", style.underline),
                ("blink", style.blink),
                ("inverse", style.inverse),
                ("invisible", style.invisible),
                ("strikethrough", style.strikethrough),
                ("overline", style.overline),
            ),
        ),
        ("resolved_foreground", _rgb(value.resolved_foreground)),
        ("resolved_background", _rgb(value.resolved_background)),
        ("selected", value.selected),
    )


def _color(value: Color) -> SnapshotValue:
    return (("kind", value.kind), ("index", value.index), ("value", _rgb(value.value)))


def _rgb(value: Rgb | None) -> SnapshotValue:
    if value is None:
        return None
    return (("r", value.r), ("g", value.g), ("b", value.b))


def _thaw(value: SnapshotValue) -> object:
    if not isinstance(value, tuple):
        return value
    if _is_snapshot_mapping(value):
        return {key: _thaw(item) for key, item in value}
    return [_thaw(item) for item in value]


def _is_snapshot_mapping(
    value: tuple[SnapshotValue, ...] | tuple[tuple[str, SnapshotValue], ...],
) -> TypeGuard[tuple[tuple[str, SnapshotValue], ...]]:
    return bool(value) and all(
        isinstance(item, tuple) and len(item) == 2 and isinstance(item[0], str) for item in value
    )


__all__ = ["FrameSnapshot", "SnapshotProfile", "normalize_frame"]
