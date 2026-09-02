import asyncio
import re
import time
from dataclasses import replace
from typing import cast

import pytest

from termtrace.duration import Duration
from termtrace.errors import (
    InvalidDurationError,
    InvalidQueryError,
    InvalidRegionError,
    LocatorTimeoutError,
    MultipleMatchesError,
    NoMatchError,
    ProtocolMismatchError,
    RequestTimeoutError,
)
from termtrace.screen.frame import (
    Appearance,
    CellStyle,
    Color,
    Colors,
    Cursor,
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
)
from termtrace.screen.locators import AsyncLocator, Locator
from termtrace.screen.queries import StyleFilter, evaluate, region, style, text
from termtrace.screen.snapshot import normalize_frame
from termtrace.session.async_ import AsyncScreen, AsyncSession
from termtrace.session.sync import Screen, Session


def _appearance(*, bold: bool = False) -> Appearance:
    default = Color("none")
    cell_style = CellStyle(
        default,
        default,
        default,
        bold,
        False,
        False,
        0,
        False,
        False,
        False,
        False,
        False,
    )
    return Appearance(cell_style, Rgb(255, 255, 255), Rgb(0, 0, 0), False)


def _frame() -> ScreenFrame:
    plain = _appearance()
    bold = _appearance(bold=True)
    row_zero_cells = (
        ScreenCell(0, 0, "A", "narrow", plain),
        ScreenCell(0, 1, "界", "wide", bold),
        ScreenCell(0, 2, "", "spacer_tail", bold),
        ScreenCell(0, 3, " ", "narrow", plain),
    )
    row_one_cells = tuple(
        ScreenCell(1, index, value, "narrow", plain) for index, value in enumerate("AB  ")
    )
    rows = (
        ScreenRow(0, True, False, "A界 ", (), row_zero_cells),
        ScreenRow(1, False, True, "AB  ", (), row_one_cells),
    )
    return ScreenFrame(
        1,
        "state",
        "projection",
        True,
        "live",
        FrameSource("actor", None, None, None, None),
        None,
        Geometry(4, 2),
        Region(0, 0, 4, 2),
        Region(0, 0, 4, 2),
        Cursor(1, 2, True, False, "block", True),
        Modes(False, False, False, True, ()),
        Colors(Rgb(255, 255, 255), Rgb(0, 0, 0), None, ()),
        rows,
        "cells",
    )


def test_screen_frame_rejects_inconsistent_coordinates_and_coverage() -> None:
    frame = _frame()
    noncontiguous_rows = (
        replace(frame.rows[0], row=1),
        frame.rows[1],
    )
    with pytest.raises(ProtocolMismatchError, match="rows are not contiguous"):
        replace(frame, rows=noncontiguous_rows)

    first_row = frame.rows[0]
    inconsistent_cells = (
        replace(first_row.cells[0], column=1),
        *first_row.cells[1:],
    )
    with pytest.raises(ProtocolMismatchError, match="cell coverage"):
        replace(frame, rows=(replace(first_row, cells=inconsistent_cells), frame.rows[1]))

    invalid_span = ScreenSpan(
        1,
        4,
        "界 ",
        first_row.cells[0].appearance,
        tuple(SpanCell(cell.grapheme, cell.wide_kind) for cell in first_row.cells[1:]),
    )
    valid_span = ScreenSpan(
        0,
        4,
        frame.rows[1].text,
        frame.rows[1].cells[0].appearance,
        tuple(SpanCell(cell.grapheme, cell.wide_kind) for cell in frame.rows[1].cells),
    )
    span_rows = (
        replace(first_row, spans=(invalid_span,), cells=()),
        replace(frame.rows[1], spans=(valid_span,), cells=()),
    )
    with pytest.raises(ProtocolMismatchError, match="spans are not contiguous"):
        replace(frame, rows=span_rows, detail="spans")


def test_exact_duration_rejects_fractional_nanoseconds_and_overflow() -> None:
    assert Duration.seconds("0.000000001").nanoseconds == 1
    with pytest.raises(InvalidDurationError):
        Duration.seconds("0.0000000001")
    with pytest.raises(InvalidDurationError):
        Duration(1 << 64)


def test_pure_queries_preserve_wide_columns_spaces_order_and_strict_cardinality() -> None:
    frame = _frame()
    match = evaluate(frame, text("界"))[0]
    assert match.ranges[0].start_column == 1
    assert match.ranges[0].end_column == 3
    assert [value.text for value in evaluate(frame, text("AB"))] == ["AB"]
    assert len(evaluate(frame, style(StyleFilter(bold=True)))) == 1

    locator = Locator(lambda request_timeout: frame, text("A"))
    with pytest.raises(MultipleMatchesError):
        locator.single()
    with pytest.raises(NoMatchError):
        Locator(lambda request_timeout: frame, text("missing")).first()
    assert locator.nth(1).ranges[0].row == 1


def test_regex_rejects_empty_and_zero_width_queries() -> None:
    with pytest.raises(InvalidQueryError):
        text("")
    with pytest.raises(InvalidQueryError):
        text(re.compile(".*?"))


def test_soft_wrap_matching_is_explicit_and_returns_per_row_ranges() -> None:
    frame = _frame()
    assert evaluate(frame, text(" AB")) == ()
    matches = evaluate(frame, text(" AB", cross_soft_wraps=True))
    assert [(item.row, item.start_column, item.end_column) for item in matches[0].ranges] == [
        (0, 3, 4),
        (1, 0, 2),
    ]


def test_partial_grapheme_matches_are_rejected_instead_of_dropped() -> None:
    frame = _frame()
    combining_row = ScreenRow(
        0,
        False,
        False,
        "e\u0301",
        (),
        (ScreenCell(0, 0, "e\u0301", "narrow", _appearance()),),
    )
    combining_frame = replace(
        frame,
        geometry=Geometry(1, 1),
        requested_region=Region(0, 0, 1, 1),
        returned_region=Region(0, 0, 1, 1),
        rows=(combining_row,),
    )
    with pytest.raises(InvalidQueryError, match="complete terminal graphemes"):
        evaluate(combining_frame, text("\u0301"))


def test_style_filters_use_wide_heads_and_accept_resolved_hex_colors() -> None:
    frame = _frame()
    head_appearance = replace(_appearance(bold=True), resolved_background=Rgb(59, 130, 246))
    row_zero = replace(
        frame.rows[0],
        cells=(
            frame.rows[0].cells[0],
            replace(frame.rows[0].cells[1], appearance=head_appearance),
            replace(frame.rows[0].cells[2], appearance=_appearance()),
            frame.rows[0].cells[3],
        ),
    )
    styled_frame = replace(frame, rows=(row_zero, frame.rows[1]))
    matches = evaluate(
        styled_frame,
        text("界") & style(StyleFilter(bold=True, background="#3b82f6")),
    )
    assert len(matches) == 1
    assert matches[0].ranges[0].start_column == 1
    assert matches[0].ranges[0].end_column == 3


def test_nested_regions_intersect_and_out_of_frame_scopes_fail() -> None:
    frame = _frame()
    spec = region(Region(0, 0, 4, 2)) & (region(Region(0, 1, 2, 1)) & text("界"))
    assert len(evaluate(frame, spec)) == 1
    with pytest.raises(InvalidRegionError):
        evaluate(frame, region(Region(0, 3, 2, 1)))


def test_locator_wait_requires_unchanged_state_and_errors_include_identity() -> None:
    frame = _frame()
    requests = 0

    def capture(request_timeout: float | None) -> ScreenFrame:
        nonlocal requests
        requests += 1
        return frame

    locator = Locator(capture, text("界"))
    result = locator.wait(
        timeout=Duration.milliseconds(100),
        interval=Duration.milliseconds(1),
        stable_for=Duration.milliseconds(3),
    )
    assert result.frame is frame
    assert requests > 1

    with pytest.raises(MultipleMatchesError) as captured:
        Locator(lambda request_timeout: frame, text("A")).single()
    assert captured.value.state_hash == frame.state_hash
    assert captured.value.ranges[0][0].row == 0


def test_sync_locator_passes_remaining_deadline_to_capture() -> None:
    observed_timeouts: list[float] = []

    def capture(request_timeout: float | None) -> ScreenFrame:
        assert request_timeout is not None
        observed_timeouts.append(request_timeout)
        time.sleep(request_timeout)
        raise RequestTimeoutError(request_timeout)

    started_at = time.monotonic()
    with pytest.raises(LocatorTimeoutError):
        Locator(capture, text("界")).wait(
            timeout=Duration.milliseconds(10),
            interval=Duration.milliseconds(1),
        )

    assert time.monotonic() - started_at < 0.2
    assert len(observed_timeouts) == 1
    assert 0 < observed_timeouts[0] <= 0.01


@pytest.mark.asyncio
async def test_async_locator_passes_remaining_deadline_to_capture() -> None:
    frame = _frame()
    observed_timeouts: list[float] = []
    capture_cancelled = False

    async def capture(request_timeout: float | None) -> ScreenFrame:
        nonlocal capture_cancelled
        assert request_timeout is not None
        observed_timeouts.append(request_timeout)
        try:
            await asyncio.Event().wait()
        finally:
            capture_cancelled = True
        return frame

    with pytest.raises(LocatorTimeoutError):
        await AsyncLocator(capture, text("界")).wait(
            timeout=Duration.milliseconds(10),
            interval=Duration.milliseconds(1),
        )

    assert capture_cancelled
    assert len(observed_timeouts) == 1
    assert 0 < observed_timeouts[0] <= 0.01


def test_screen_locator_forwards_remaining_deadline_to_frame_request() -> None:
    request_timeouts: list[float | None] = []

    class SilentSession:
        def request(
            self,
            method: dict[str, object],
            response_type: type[object],
            *,
            request_id: str | None = None,
            request_timeout: float | None = None,
        ) -> object:
            request_timeouts.append(request_timeout)
            assert request_timeout is not None
            raise RequestTimeoutError(request_timeout)

    screen = Screen(cast(Session, SilentSession()))
    with pytest.raises(LocatorTimeoutError):
        screen.get_by_text("界").wait(
            timeout=Duration.milliseconds(10),
            interval=Duration.milliseconds(1),
        )

    assert len(request_timeouts) == 1
    assert request_timeouts[0] is not None
    assert 0 < request_timeouts[0] <= 0.01


@pytest.mark.asyncio
async def test_async_screen_locator_cancels_deadline_bounded_frame_request() -> None:
    request_timeouts: list[float | None] = []
    request_cancelled = False

    class SilentSession:
        async def request(
            self,
            method: dict[str, object],
            response_type: type[object],
            *,
            request_id: str | None = None,
            request_timeout: float | None = None,
        ) -> object:
            nonlocal request_cancelled
            request_timeouts.append(request_timeout)
            try:
                await asyncio.Event().wait()
            finally:
                request_cancelled = True
            raise AssertionError("capture unexpectedly completed")

    screen = AsyncScreen(cast(AsyncSession, SilentSession()))
    with pytest.raises(LocatorTimeoutError):
        await screen.get_by_text("界").wait(
            timeout=Duration.milliseconds(10),
            interval=Duration.milliseconds(1),
        )

    assert request_cancelled
    assert len(request_timeouts) == 1
    assert request_timeouts[0] is not None
    assert 0 < request_timeouts[0] <= 0.01


def test_snapshot_profiles_exclude_identity_and_retain_visual_facts() -> None:
    frame = _frame()
    snapshot = normalize_frame(frame, "cells").to_dict()
    assert "state_hash" not in snapshot
    assert "projection_hash" not in snapshot
    assert snapshot["geometry"] == {"cols": 4, "rows": 2}
    rows = snapshot["rows"]
    assert isinstance(rows, list)
    first_row = rows[0]
    assert isinstance(first_row, dict)
    cells = first_row["cells"]
    assert isinstance(cells, list)
    second_cell = cells[1]
    assert isinstance(second_cell, dict)
    assert second_cell["wide_kind"] == "wide"
    assert snapshot["capture_quality"] == "live"
    assert snapshot["colors"] == {
        "foreground": {"r": 255, "g": 255, "b": 255},
        "background": {"r": 0, "g": 0, "b": 0},
        "cursor": None,
        "palette": [],
    }

    changed_quality = normalize_frame(replace(frame, capture_quality="incomplete"), "cells")
    changed_colors = normalize_frame(
        replace(
            frame,
            colors=replace(frame.colors, foreground=Rgb(128, 128, 128)),
        ),
        "cells",
    )
    assert changed_quality != normalize_frame(frame, "cells")
    assert changed_colors != normalize_frame(frame, "cells")
