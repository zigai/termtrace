import asyncio
import re
import time
from dataclasses import replace
from pathlib import Path

import msgspec
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
from termtrace.transport.generated import driver_response as response_wire

FIXTURE_PATH = Path(__file__).parents[3] / "fixtures" / "conformance" / "locators" / "locator.json"


class ConformanceMatch(msgspec.Struct):
    text: str
    ranges: list[list[int]]


class _TextQuery(msgspec.Struct, tag_field="kind", tag="text"):
    value: str
    exact: bool = False
    cross_soft_wraps: bool = False


class _RegexQuery(msgspec.Struct, tag_field="kind", tag="regex"):
    pattern: str


class _StyleQuery(msgspec.Struct, tag_field="kind", tag="style"):
    bold: bool | None = None
    resolved_background: str | None = None


class _RowQuery(msgspec.Struct, tag_field="kind", tag="row"):
    row: int


class _RegionQuery(msgspec.Struct, tag_field="kind", tag="region"):
    top: int
    left: int
    width: int
    height: int


class _AllQuery(msgspec.Struct, tag_field="kind", tag="all"):
    specs: list[object]


_FixtureQuery = _TextQuery | _RegexQuery | _StyleQuery | _RowQuery | _RegionQuery | _AllQuery


class ConformanceCase(msgspec.Struct):
    name: str
    query: _FixtureQuery
    matches: list[ConformanceMatch] | None = None
    error: str | None = None


class LocatorCorpus(msgspec.Struct):
    version: int
    frame: response_wire.ScreenFrame
    cases: list[ConformanceCase]


def _spec(value: _FixtureQuery | LocatorSpec | object) -> LocatorSpec:
    if isinstance(value, LocatorSpec):
        return value
    parsed = (
        value
        if isinstance(
            value, (_TextQuery, _RegexQuery, _StyleQuery, _RowQuery, _RegionQuery, _AllQuery)
        )
        else msgspec.convert(value, type=_FixtureQuery, strict=True)
    )
    if isinstance(parsed, _TextQuery):
        return text(
            parsed.value,
            exact=parsed.exact,
            cross_soft_wraps=parsed.cross_soft_wraps,
        )
    if isinstance(parsed, _RegexQuery):
        return text(re.compile(parsed.pattern))
    if isinstance(parsed, _StyleQuery):
        return style(StyleFilter(bold=parsed.bold, background=parsed.resolved_background))
    if isinstance(parsed, _RowQuery):
        return row(parsed.row)
    if isinstance(parsed, _RegionQuery):
        return region(
            Region(
                top=parsed.top,
                left=parsed.left,
                width=parsed.width,
                height=parsed.height,
            )
        )
    return AllSpec(tuple(_spec(item) for item in parsed.specs))


def _normalized_matches(
    frame: ScreenFrame, query: _FixtureQuery | LocatorSpec | object
) -> list[ConformanceMatch]:
    matches = evaluate(frame, _spec(query))
    normalized: list[ConformanceMatch] = []
    for match in matches:
        assert match.frame is frame
        assert match.state_hash == "fixture-state-v1"
        assert match.projection_hash == "fixture-frame-v1"
        assert match.frame.presented is False
        assert len(match.query) <= 256
        ranges: list[list[int]] = [
            [value.row, value.start_column, value.end_column] for value in match.ranges
        ]
        normalized.append(ConformanceMatch(text=match.text, ranges=ranges))
    return normalized


def test_shared_locator_conformance_corpus_matches_public_contract() -> None:
    corpus = msgspec.json.decode(FIXTURE_PATH.read_bytes(), type=LocatorCorpus)
    assert corpus.version == 1
    frame = normalize_frame(corpus.frame)
    assert frame.presented is False
    assert frame.capture_quality == "incomplete"

    for test_case in corpus.cases:
        if test_case.error == "invalid_query":
            with pytest.raises(InvalidQueryError):
                _normalized_matches(frame, test_case.query)
            continue
        assert test_case.error is None
        assert test_case.matches is not None
        assert _normalized_matches(frame, test_case.query) == test_case.matches, test_case.name


def test_runner_neutral_style_wrap_and_region_assertions() -> None:
    corpus = msgspec.json.decode(FIXTURE_PATH.read_bytes(), type=LocatorCorpus)
    frame = normalize_frame(corpus.frame)
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
    corpus = msgspec.json.decode(FIXTURE_PATH.read_bytes(), type=LocatorCorpus)
    return normalize_frame(corpus.frame)


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
