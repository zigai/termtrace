import asyncio
import json
import re
import time
from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path

import pytest

from termtrace.duration import Duration
from termtrace.errors import InvalidQueryError, InvalidRegionError, LocatorTimeoutError
from termtrace.screen.frame import Region, ScreenFrame
from termtrace.screen.locators import AsyncLocator, Locator
from termtrace.screen.queries import (
    AllSpec,
    LocatorSpec,
    StyleFilter,
    evaluate,
    region,
    row,
    style,
    text,
)
from termtrace.screen.wire import normalize_frame
from termtrace.testing.assertions import (
    assert_match_style,
    assert_neutral_region,
    assert_no_matches,
    assert_no_wrapped_rows,
)

FIXTURE_PATH = Path(__file__).parents[3] / "fixtures" / "conformance" / "locators" / "locator.json"


def _mapping(value: object) -> Mapping[str, object]:
    assert isinstance(value, dict)
    assert all(isinstance(key, str) for key in value)
    return value


def _integer(value: object) -> int:
    assert isinstance(value, int)
    return value


def _string(value: object) -> str:
    assert isinstance(value, str)
    return value


def _spec(value: object) -> LocatorSpec:
    query = _mapping(value)
    kind = _string(query["kind"])
    if kind == "text":
        exact = query.get("exact", False)
        cross_soft_wraps = query.get("cross_soft_wraps", False)
        assert isinstance(exact, bool)
        assert isinstance(cross_soft_wraps, bool)
        return text(
            _string(query["value"]),
            exact=exact,
            cross_soft_wraps=cross_soft_wraps,
        )
    if kind == "regex":
        return text(re.compile(_string(query["pattern"])))
    if kind == "style":
        bold = query.get("bold")
        resolved_background = query.get("resolved_background")
        assert bold is None or isinstance(bold, bool)
        assert resolved_background is None or isinstance(resolved_background, str)
        return style(StyleFilter(bold=bold, background=resolved_background))
    if kind == "row":
        return row(_integer(query["row"]))
    if kind == "region":
        return region(
            Region(
                top=_integer(query["top"]),
                left=_integer(query["left"]),
                width=_integer(query["width"]),
                height=_integer(query["height"]),
            )
        )
    if kind == "all":
        specs = query["specs"]
        assert isinstance(specs, list)
        return AllSpec(tuple(_spec(item) for item in specs))
    raise AssertionError(f"unknown fixture query kind {kind}")


def _normalized_matches(frame: ScreenFrame, query: object) -> list[dict[str, object]]:
    matches = evaluate(frame, _spec(query))
    normalized: list[dict[str, object]] = []
    for match in matches:
        assert match.frame is frame
        assert match.state_hash == "fixture-state-v1"
        assert match.projection_hash == "fixture-frame-v1"
        assert match.frame.presented is False
        assert len(match.query) <= 256
        ranges: list[list[int]] = [
            [value.row, value.start_column, value.end_column] for value in match.ranges
        ]
        normalized.append({"text": match.text, "ranges": ranges})
    return normalized


def test_shared_locator_conformance_corpus_matches_public_contract() -> None:
    payload: object = json.loads(FIXTURE_PATH.read_text())
    corpus = _mapping(payload)
    assert corpus["version"] == 1
    frame = normalize_frame(_mapping(corpus["frame"]))
    assert frame.presented is False
    assert frame.capture_quality == "incomplete"
    cases = corpus["cases"]
    assert isinstance(cases, list)

    for value in cases:
        test_case = _mapping(value)
        error = test_case.get("error")
        if error == "invalid_query":
            with pytest.raises(InvalidQueryError):
                _normalized_matches(frame, test_case["query"])
            continue
        assert error is None
        expected = test_case.get("matches")
        assert isinstance(expected, list)
        assert _normalized_matches(frame, test_case["query"]) == expected, test_case["name"]


def test_runner_neutral_style_wrap_and_region_assertions() -> None:
    payload: object = json.loads(FIXTURE_PATH.read_text())
    corpus = _mapping(payload)
    frame = normalize_frame(_mapping(corpus["frame"]))
    matches = evaluate(frame, text("ERROR"))
    assert len(matches) == 1
    assert_match_style(matches[0], StyleFilter(bold=True))
    with pytest.raises(AssertionError):
        assert_match_style(matches[0], StyleFilter(bold=False))
    with pytest.raises(AssertionError):
        assert_no_wrapped_rows(frame)
    assert_neutral_region(frame, Region(top=0, left=11, width=1, height=1))
    with pytest.raises(AssertionError):
        assert_neutral_region(frame, Region(top=2, left=0, width=12, height=1))


def _fixture_frame() -> ScreenFrame:
    payload: object = json.loads(FIXTURE_PATH.read_text())
    return normalize_frame(_mapping(_mapping(payload)["frame"]))


def test_row_query_must_be_inside_returned_projection() -> None:
    frame = _fixture_frame()
    returned_region = Region(top=1, left=0, width=frame.geometry.cols, height=1)
    projected = replace(
        frame,
        requested_region=returned_region,
        returned_region=returned_region,
        rows=(frame.rows[1],),
    )

    assert evaluate(projected, row(1))
    with pytest.raises(InvalidRegionError, match="query row is outside the returned frame region"):
        evaluate(projected, row(0))
    with pytest.raises(InvalidRegionError, match="query row is outside the returned frame region"):
        evaluate(projected, row(2))


def test_text_only_frames_reject_queries_as_insufficient_detail() -> None:
    frame = _fixture_frame()
    text_frame = replace(
        frame,
        detail="text",
        rows=tuple(replace(value, spans=(), cells=()) for value in frame.rows),
    )

    with pytest.raises(InvalidQueryError, match="query requires spans or cells frame detail"):
        evaluate(text_frame, text("ERROR"))
    with pytest.raises(InvalidQueryError, match="query requires spans or cells frame detail"):
        text_frame.query(text("ERROR")).all()
    with pytest.raises(InvalidQueryError, match="query requires spans or cells frame detail"):
        assert_no_matches(text_frame, text("ERROR"))


def test_conflicting_composed_row_scopes_are_invalid() -> None:
    frame = _fixture_frame()

    with pytest.raises(InvalidQueryError, match="composed row scopes conflict"):
        evaluate(frame, row(0) & row(1))
    with pytest.raises(InvalidQueryError, match="composed row scopes conflict"):
        assert_no_matches(frame, row(0) & row(1))


def test_sync_locator_rejects_matching_frame_captured_after_deadline() -> None:
    frame = _fixture_frame()

    def capture(request_timeout: float | None) -> ScreenFrame:
        assert request_timeout is not None
        time.sleep(0.02)
        return frame

    locator = Locator(capture, text("ERROR"))
    with pytest.raises(LocatorTimeoutError) as captured:
        locator.wait(
            timeout=Duration.milliseconds(5),
            interval=Duration.milliseconds(1),
        )

    assert captured.value.last_frame is frame


@pytest.mark.asyncio
async def test_async_locator_cancels_capture_at_deadline() -> None:
    frame = _fixture_frame()
    capture_started = False
    capture_cancelled = False

    async def capture(request_timeout: float | None) -> ScreenFrame:
        nonlocal capture_started, capture_cancelled
        assert request_timeout is not None
        capture_started = True
        try:
            await asyncio.Event().wait()
        finally:
            capture_cancelled = True
        return frame

    locator = AsyncLocator(capture, text("ERROR"))
    with pytest.raises(LocatorTimeoutError) as captured:
        await locator.wait(
            timeout=Duration.milliseconds(5),
            interval=Duration.milliseconds(1),
        )

    assert capture_started
    assert capture_cancelled
    assert captured.value.last_frame is None
