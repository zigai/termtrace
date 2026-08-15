"""Immutable screen query specifications and pure frame evaluation."""

from __future__ import annotations

from dataclasses import dataclass
from re import Pattern

from termtrace.errors import (
    InvalidQueryError,
    InvalidRegionError,
    MultipleMatchesError,
    NoMatchError,
    ResourceLimitError,
)
from termtrace.screen.frame import Color, Region, Rgb, ScreenCell, ScreenFrame

MAX_QUERY_MATCHES = 10_000


@dataclass(frozen=True, slots=True)
class CellRange:
    row: int
    start_column: int
    end_column: int


@dataclass(frozen=True, slots=True)
class StyleFilter:
    bold: bool | None = None
    faint: bool | None = None
    italic: bool | None = None
    underline: int | None = None
    blink: bool | None = None
    inverse: bool | None = None
    invisible: bool | None = None
    strikethrough: bool | None = None
    overline: bool | None = None
    selected: bool | None = None
    wide_kind: str | None = None
    foreground: tuple[int, int, int] | str | None = None
    background: tuple[int, int, int] | str | None = None
    source_foreground: Color | None = None
    source_background: Color | None = None
    source_underline_color: Color | None = None

    def __post_init__(self) -> None:
        for name in ("foreground", "background"):
            value = getattr(self, name)
            if isinstance(value, str):
                object.__setattr__(self, name, _parse_hex_rgb(value))
                continue
            if value is None:
                continue
            if (
                len(value) != 3
                or any(isinstance(channel, bool) for channel in value)
                or any(not isinstance(channel, int) for channel in value)
                or any(channel < 0 or channel > 255 for channel in value)
            ):
                raise InvalidQueryError(f"{name} must be #RRGGBB or three RGB byte values")


def _parse_hex_rgb(value: str) -> tuple[int, int, int]:
    if len(value) != 7 or not value.startswith("#"):
        raise InvalidQueryError("resolved colors must use #RRGGBB syntax")
    try:
        return (int(value[1:3], 16), int(value[3:5], 16), int(value[5:7], 16))
    except ValueError as error:
        raise InvalidQueryError("resolved colors must use #RRGGBB syntax") from error


class LocatorSpec:
    """Base type for immutable locator specifications."""

    def __and__(self, other: LocatorSpec) -> LocatorSpec:
        return AllSpec((self, other))


@dataclass(frozen=True, slots=True)
class TextSpec(LocatorSpec):
    query: str | Pattern[str]
    exact: bool = False
    cross_soft_wraps: bool = False

    def __post_init__(self) -> None:
        if isinstance(self.query, str):
            if not self.query:
                raise InvalidQueryError("text query must not be empty")
            return
        if self.query.search("") is not None:
            raise InvalidQueryError("regular expression must not produce zero-width matches")


@dataclass(frozen=True, slots=True)
class StyleSpec(LocatorSpec):
    style: StyleFilter


@dataclass(frozen=True, slots=True)
class RowSpec(LocatorSpec):
    row: int

    def __post_init__(self) -> None:
        if self.row < 0:
            raise InvalidQueryError("row must be nonnegative")


@dataclass(frozen=True, slots=True)
class RegionSpec(LocatorSpec):
    region: Region


@dataclass(frozen=True, slots=True)
class AllSpec(LocatorSpec):
    specs: tuple[LocatorSpec, ...]

    def __post_init__(self) -> None:
        if not self.specs:
            raise InvalidQueryError("composed query must contain at least one specification")

    def __and__(self, other: LocatorSpec) -> LocatorSpec:
        return AllSpec((*self.specs, other))


@dataclass(frozen=True, slots=True)
class ScreenMatch:
    frame: ScreenFrame
    ranges: tuple[CellRange, ...]
    text: str
    query: str

    @property
    def state_hash(self) -> str:
        return self.frame.state_hash

    @property
    def projection_hash(self) -> str:
        return self.frame.projection_hash


def text(
    query: str | Pattern[str], *, exact: bool = False, cross_soft_wraps: bool = False
) -> TextSpec:
    return TextSpec(query, exact, cross_soft_wraps)


def row(index: int) -> RowSpec:
    return RowSpec(index)


def region(value: Region) -> RegionSpec:
    return RegionSpec(value)


def style(value: StyleFilter) -> StyleSpec:
    return StyleSpec(value)


def evaluate(frame: ScreenFrame, spec: LocatorSpec) -> tuple[ScreenMatch, ...]:
    if frame.detail == "text":
        raise InvalidQueryError("query requires spans or cells frame detail")
    rows, selected_region, style_filter, text_spec = _flatten(spec)
    if selected_region is not None:
        _validate_region(frame, selected_region)
    if rows is not None:
        if any(row_index < 0 or row_index >= frame.geometry.rows for row_index in rows):
            raise InvalidRegionError("query row is outside the frame geometry")
        if any(
            row_index < frame.returned_region.top or row_index >= frame.returned_region.bottom
            for row_index in rows
        ):
            raise InvalidRegionError("query row is outside the returned frame region")
    if text_spec is not None and text_spec.cross_soft_wraps:
        return _wrapped_matches(
            frame, rows, selected_region, style_filter, text_spec, describe(spec)
        )
    matches: list[ScreenMatch] = []
    for screen_row in frame.rows:
        if rows is not None and screen_row.row not in rows:
            continue
        cells = screen_row.expanded_cells()
        if selected_region is not None:
            cells = tuple(cell for cell in cells if selected_region.contains(cell.row, cell.column))
        if text_spec is not None:
            row_matches = _text_matches(frame, cells, text_spec, describe(spec))
            matches.extend(
                screen_match
                for screen_match in row_matches
                if style_filter is None or _match_has_style(screen_match, style_filter)
            )
            _check_match_limit(len(matches))
            continue
        if style_filter is not None:
            cells = tuple(
                cell
                for cell in cells
                if _is_style_candidate(cell, style_filter) and _matches_style(cell, style_filter)
            )
        matches.extend(_contiguous_matches(frame, cells, describe(spec)))
        _check_match_limit(len(matches))
    matches.sort(
        key=lambda match: (
            match.ranges[0].row,
            match.ranges[0].start_column,
            match.ranges[-1].end_column,
        )
    )
    return tuple(matches)


def _flatten(
    spec: LocatorSpec,
) -> tuple[set[int] | None, Region | None, StyleFilter | None, TextSpec | None]:
    specs: list[LocatorSpec] = []
    pending: list[LocatorSpec] = [spec]
    while pending:
        item = pending.pop(0)
        if isinstance(item, AllSpec):
            pending[0:0] = item.specs
            continue
        specs.append(item)
    rows: set[int] | None = None
    selected_region: Region | None = None
    style_filter: StyleFilter | None = None
    text_spec: TextSpec | None = None
    for item in specs:
        if isinstance(item, RowSpec):
            if rows is not None and item.row not in rows:
                raise InvalidQueryError("composed row scopes conflict")
            rows = {item.row}
            continue
        if isinstance(item, RegionSpec):
            selected_region = _intersect_regions(selected_region, item.region)
            continue
        if isinstance(item, StyleSpec):
            if style_filter is not None:
                raise InvalidQueryError("only one style filter may be composed")
            style_filter = item.style
            continue
        if isinstance(item, TextSpec):
            if text_spec is not None:
                raise InvalidQueryError("only one text query may be composed")
            text_spec = item
    return rows, selected_region, style_filter, text_spec


def _wrapped_matches(
    frame: ScreenFrame,
    rows: set[int] | None,
    selected_region: Region | None,
    style_filter: StyleFilter | None,
    spec: TextSpec,
    query: str,
) -> tuple[ScreenMatch, ...]:
    groups: list[list[tuple[ScreenCell, ...]]] = []
    current: list[tuple[ScreenCell, ...]] = []
    previous_soft = False
    for screen_row in frame.rows:
        if rows is not None and screen_row.row not in rows:
            continue
        cells = screen_row.expanded_cells()
        if selected_region is not None:
            cells = tuple(cell for cell in cells if selected_region.contains(cell.row, cell.column))
        if current and not (previous_soft and screen_row.wrap_continuation):
            groups.append(current)
            current = []
        current.append(cells)
        previous_soft = screen_row.soft_wrapped
    if current:
        groups.append(current)
    results: list[ScreenMatch] = []
    for group in groups:
        streams: list[str] = []
        offsets: list[tuple[int, dict[int, tuple[int, int]], tuple[ScreenCell, ...]]] = []
        total = 0
        for cells in group:
            stream, boundaries = _cell_stream(cells)
            streams.append(stream)
            offsets.append((total, boundaries, cells))
            total += len(stream)
        combined = "".join(streams)
        spans: tuple[tuple[int, int], ...]
        if spec.exact:
            spans = ((0, len(combined)),) if _full_match(combined, spec.query) else ()
        elif isinstance(spec.query, str):
            found: list[tuple[int, int]] = []
            start_at = 0
            while True:
                found_at = combined.find(spec.query, start_at)
                if found_at < 0:
                    break
                found.append((found_at, found_at + len(spec.query)))
                start_at = found_at + 1
            spans = tuple(found)
        else:
            spans = tuple((match.start(), match.end()) for match in spec.query.finditer(combined))
        for start, end in spans:
            if start == end:
                raise InvalidQueryError("regular expression produced a zero-width match")
            covered_ranges: list[CellRange] = []
            for base, boundaries, cells in offsets:
                local_start = max(start - base, 0)
                local_end = min(end - base, len(_cell_stream(cells)[0]))
                if local_start >= local_end:
                    continue
                if local_start not in boundaries or local_end not in boundaries:
                    raise InvalidQueryError(
                        "text match is not aligned to complete terminal graphemes"
                    )
                first = boundaries[local_start][0]
                after = boundaries[local_end][1]
                covered_ranges.append(CellRange(cells[0].row, first, after))
            if covered_ranges:
                screen_match = ScreenMatch(frame, tuple(covered_ranges), combined[start:end], query)
                if style_filter is None or _match_has_style(screen_match, style_filter):
                    results.append(screen_match)
                    _check_match_limit(len(results))
    results.sort(
        key=lambda match: (
            match.ranges[0].row,
            match.ranges[0].start_column,
            match.ranges[-1].end_column,
        )
    )
    return tuple(results)


def _check_match_limit(count: int) -> None:
    if count <= MAX_QUERY_MATCHES:
        return
    raise ResourceLimitError(
        f"query produced {count} matches, exceeding the {MAX_QUERY_MATCHES} match limit",
        details={
            "kind": "match_count",
            "requested": count,
            "maximum": MAX_QUERY_MATCHES,
        },
    )


def _intersect_regions(first: Region | None, second: Region) -> Region:
    if first is None:
        return second
    top = max(first.top, second.top)
    left = max(first.left, second.left)
    bottom = min(first.bottom, second.bottom)
    right = min(first.right, second.right)
    if top >= bottom or left >= right:
        raise InvalidQueryError("composed regions do not intersect")
    return Region(top, left, right - left, bottom - top)


def _contiguous_matches(
    frame: ScreenFrame, cells: tuple[ScreenCell, ...], query: str
) -> list[ScreenMatch]:
    if not cells:
        return []
    results: list[ScreenMatch] = []
    start = 0
    for index in range(1, len(cells) + 1):
        if index < len(cells) and cells[index].column == cells[index - 1].column + 1:
            continue
        group = cells[start:index]
        results.append(_make_match(frame, group, query))
        start = index
    return results


def _text_matches(
    frame: ScreenFrame, cells: tuple[ScreenCell, ...], spec: TextSpec, query: str
) -> list[ScreenMatch]:
    stream, boundaries = _cell_stream(cells)
    spans: tuple[tuple[int, int], ...]
    if spec.exact:
        spans = ((0, len(stream)),) if _full_match(stream, spec.query) else ()
    elif isinstance(spec.query, str):
        found: list[tuple[int, int]] = []
        start = 0
        while True:
            index = stream.find(spec.query, start)
            if index < 0:
                break
            found.append((index, index + len(spec.query)))
            start = index + 1
        spans = tuple(found)
    else:
        spans = tuple((match.start(), match.end()) for match in spec.query.finditer(stream))
    results: list[ScreenMatch] = []
    for start, end in spans:
        if start == end:
            raise InvalidQueryError("regular expression produced a zero-width match")
        if start not in boundaries or end not in boundaries:
            raise InvalidQueryError("text match is not aligned to complete terminal graphemes")
        first, after = boundaries[start][0], boundaries[end][1]
        covered = tuple(cell for cell in cells if first <= cell.column < after)
        if covered:
            results.append(_make_match(frame, covered, query, stream[start:end]))
    return results


def _cell_stream(
    cells: tuple[ScreenCell, ...],
) -> tuple[str, dict[int, tuple[int, int]]]:
    parts: list[str] = []
    boundaries: dict[int, tuple[int, int]] = {
        0: (cells[0].column if cells else 0, cells[0].column if cells else 0)
    }
    offset = 0
    index = 0
    while index < len(cells):
        cell = cells[index]
        grapheme = cell.grapheme
        if not grapheme and cell.wide_kind == "narrow":
            grapheme = " "
        if not grapheme:
            index += 1
            continue
        end_column = cell.column + 1
        following = index + 1
        while (
            following < len(cells)
            and cells[following].wide_kind in ("spacer_tail", "spacer_head")
            and not cells[following].grapheme
        ):
            end_column = cells[following].column + 1
            following += 1
        boundaries[offset] = (cell.column, cell.column)
        parts.append(grapheme)
        offset += len(grapheme)
        boundaries[offset] = (end_column, end_column)
        index = following
    return "".join(parts), boundaries


def _validate_region(frame: ScreenFrame, selected: Region) -> None:
    if selected.right > frame.geometry.cols or selected.bottom > frame.geometry.rows:
        raise InvalidRegionError("query region is outside the frame geometry")
    returned = frame.returned_region
    if (
        selected.top < returned.top
        or selected.left < returned.left
        or selected.right > returned.right
        or selected.bottom > returned.bottom
    ):
        raise InvalidRegionError("query region is outside the returned frame region")


def _match_has_style(screen_match: ScreenMatch, expected: StyleFilter) -> bool:
    covered = False
    for match_range in screen_match.ranges:
        row_offset = match_range.row - screen_match.frame.returned_region.top
        if row_offset < 0 or row_offset >= len(screen_match.frame.rows):
            return False
        row_value = screen_match.frame.rows[row_offset]
        if row_value.row != match_range.row:
            return False
        for cell in row_value.expanded_cells():
            if (
                cell.column < match_range.start_column
                or cell.column >= match_range.end_column
                or not _is_style_candidate(cell, expected)
            ):
                continue
            covered = True
            if not _matches_style(cell, expected):
                return False
    return covered


def _full_match(value: str, query: str | Pattern[str]) -> bool:
    if isinstance(query, str):
        return value == query
    return query.fullmatch(value) is not None


def _make_match(
    frame: ScreenFrame,
    cells: tuple[ScreenCell, ...],
    query: str,
    matched_text: str | None = None,
) -> ScreenMatch:
    first = cells[0]
    last = cells[-1]
    text_value = (
        matched_text
        if matched_text is not None
        else "".join(cell.grapheme or (" " if cell.wide_kind == "narrow" else "") for cell in cells)
    )
    end_column = last.column + (2 if last.wide_kind == "wide" else 1)
    return ScreenMatch(frame, (CellRange(first.row, first.column, end_column),), text_value, query)


def _is_style_candidate(cell: ScreenCell, expected: StyleFilter) -> bool:
    if cell.wide_kind not in ("spacer_tail", "spacer_head"):
        return True
    return expected.wide_kind == cell.wide_kind


def _matches_style(cell: ScreenCell, expected: StyleFilter) -> bool:
    appearance = cell.appearance
    source = appearance.style
    attributes = (
        (expected.bold, source.bold),
        (expected.faint, source.faint),
        (expected.italic, source.italic),
        (expected.underline, source.underline),
        (expected.blink, source.blink),
        (expected.inverse, source.inverse),
        (expected.invisible, source.invisible),
        (expected.strikethrough, source.strikethrough),
        (expected.overline, source.overline),
    )
    if any(wanted is not None and actual != wanted for wanted, actual in attributes):
        return False
    if expected.selected is not None and appearance.selected != expected.selected:
        return False
    if expected.wide_kind is not None and cell.wide_kind != expected.wide_kind:
        return False
    if (
        expected.foreground is not None
        and _rgb_tuple(appearance.resolved_foreground) != expected.foreground
    ):
        return False
    if (
        expected.background is not None
        and _rgb_tuple(appearance.resolved_background) != expected.background
    ):
        return False
    if expected.source_foreground is not None and source.foreground != expected.source_foreground:
        return False
    if expected.source_background is not None and source.background != expected.source_background:
        return False
    if (
        expected.source_underline_color is not None
        and source.underline_color != expected.source_underline_color
    ):
        return False
    return True


def match_has_style(match: ScreenMatch, expected: StyleFilter) -> bool:
    """Return whether every authoritative head cell covered by a match has the style."""
    covered: list[ScreenCell] = []
    for screen_row in match.frame.rows:
        for cell in screen_row.expanded_cells():
            if not _is_style_candidate(cell, expected):
                continue
            if any(
                cell.row == cell_range.row
                and cell_range.start_column <= cell.column < cell_range.end_column
                for cell_range in match.ranges
            ):
                covered.append(cell)
    return bool(covered) and all(_matches_style(cell, expected) for cell in covered)


def _rgb_tuple(value: Rgb | None) -> tuple[int, int, int] | None:
    if value is None:
        return None
    return (value.r, value.g, value.b)


def describe(spec: LocatorSpec) -> str:
    value = repr(spec)
    return value if len(value) <= 256 else f"{value[:255]}…"


@dataclass(frozen=True, slots=True)
class Query:
    frame: ScreenFrame
    spec: LocatorSpec

    def all(self) -> tuple[ScreenMatch, ...]:
        return evaluate(self.frame, self.spec)

    def count(self) -> int:
        return len(self.all())

    def single(self) -> ScreenMatch:
        matches = self.all()
        if not matches:
            raise NoMatchError("query did not match this frame", state_hash=self.frame.state_hash)
        if len(matches) != 1:
            raise MultipleMatchesError(
                len(matches),
                [match.text for match in matches],
                state_hash=self.frame.state_hash,
                ranges=[match.ranges for match in matches],
            )
        return matches[0]

    def first(self) -> ScreenMatch:
        matches = self.all()
        if not matches:
            raise NoMatchError("query did not match this frame", state_hash=self.frame.state_hash)
        return matches[0]

    def nth(self, index: int) -> ScreenMatch:
        if index < 0:
            raise InvalidQueryError("match index must be nonnegative")
        matches = self.all()
        if index >= len(matches):
            raise NoMatchError(
                f"query has no match at zero-based index {index}",
                state_hash=self.frame.state_hash,
            )
        return matches[index]


__all__ = [
    "AllSpec",
    "CellRange",
    "LocatorSpec",
    "Query",
    "RegionSpec",
    "RowSpec",
    "ScreenMatch",
    "StyleFilter",
    "StyleSpec",
    "TextSpec",
    "evaluate",
    "describe",
    "match_has_style",
    "region",
    "row",
    "style",
    "text",
]
