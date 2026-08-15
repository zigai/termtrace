"""Runner-neutral assertions over immutable rich frame evidence."""

from __future__ import annotations

import json
import re
from pathlib import Path
from re import Pattern
from typing import Protocol

from termtrace.screen.frame import Region, ScreenFrame
from termtrace.screen.queries import (
    LocatorSpec,
    ScreenMatch,
    StyleFilter,
    evaluate,
    match_has_style,
    region,
)
from termtrace.screen.snapshot import FrameSnapshot, SnapshotProfile, normalize_frame
from termtrace.session.actions import StableCapture


class _ScreenAccess(Protocol):
    def frame(self) -> ScreenFrame: ...

    def capture_stable(self, *, stable_for: float) -> StableCapture: ...


class _ScreenSubject(Protocol):
    @property
    def screen(self) -> _ScreenAccess: ...


def assert_matches(
    frame: ScreenFrame, spec: LocatorSpec, *, count: int | None = None
) -> tuple[ScreenMatch, ...]:
    matches = evaluate(frame, spec)
    if count is None and not matches:
        raise AssertionError(f"expected query to match; query={spec!r}; state={frame.state_hash}")
    if count is not None and len(matches) != count:
        excerpts = tuple(match.text[:80] for match in matches[:8])
        raise AssertionError(
            f"expected {count} matches, got {len(matches)}; excerpts={excerpts!r}; state={frame.state_hash}"
        )
    return matches


def assert_no_matches(frame: ScreenFrame, spec: LocatorSpec) -> None:
    matches = evaluate(frame, spec)
    if matches:
        excerpts = tuple(match.text[:80] for match in matches[:8])
        raise AssertionError(
            f"expected no matches, got {len(matches)}; excerpts={excerpts!r}; state={frame.state_hash}"
        )


def assert_frame_text(frame: ScreenFrame, expected: str | Pattern[str]) -> None:
    if isinstance(expected, str):
        if expected not in frame.text:
            raise AssertionError(
                f"expected screen text to contain {expected!r}; actual={frame.text[:512]!r}"
            )
        return
    if expected.search(frame.text) is None:
        raise AssertionError(
            f"expected screen text to match {expected.pattern!r}; actual={frame.text[:512]!r}"
        )


def assert_match_text(match: ScreenMatch, expected: str | Pattern[str]) -> None:
    if isinstance(expected, str) and match.text != expected:
        raise AssertionError(f"expected matched text {expected!r}, got {match.text[:512]!r}")
    if not isinstance(expected, str) and expected.fullmatch(match.text) is None:
        raise AssertionError(
            f"matched text {match.text[:512]!r} does not satisfy {expected.pattern!r}"
        )


def assert_match_style(match: ScreenMatch, expected: StyleFilter) -> None:
    if not match_has_style(match, expected):
        raise AssertionError(
            f"expected every covered cell to match {expected!r}; state={match.state_hash}"
        )


def assert_no_wrapped_rows(frame: ScreenFrame) -> None:
    wrapped: list[int] = [
        screen_row.row
        for screen_row in frame.rows
        if screen_row.soft_wrapped or screen_row.wrap_continuation
    ]
    if wrapped:
        raise AssertionError(
            f"expected no native wrapped rows, got {wrapped!r}; state={frame.state_hash}"
        )


def assert_neutral_region(frame: ScreenFrame, area: Region) -> None:
    evaluate(frame, region(area))
    checked = 0
    non_neutral = 0
    for screen_row in frame.rows:
        for cell in screen_row.expanded_cells():
            if not area.contains(cell.row, cell.column):
                continue
            checked += 1
            source = cell.appearance.style
            neutral = (
                cell.grapheme in ("", " ")
                and cell.wide_kind in ("narrow", "spacer_head", "spacer_tail")
                and source.foreground.kind == "none"
                and source.background.kind == "none"
                and source.underline_color.kind == "none"
                and not source.bold
                and not source.faint
                and not source.italic
                and source.underline == 0
                and not source.blink
                and not source.inverse
                and not source.invisible
                and not source.strikethrough
                and not source.overline
                and not cell.appearance.selected
                and cell.appearance.resolved_foreground is None
                and cell.appearance.resolved_background is None
            )
            if not neutral:
                non_neutral += 1
    expected = area.width * area.height
    if checked != expected or non_neutral:
        raise AssertionError(
            f"expected {expected} neutral cells, checked {checked} with "
            f"{non_neutral} non-neutral; state={frame.state_hash}"
        )


def assert_normalized_frame(
    frame: ScreenFrame, expected: FrameSnapshot, profile: SnapshotProfile | None = None
) -> FrameSnapshot:
    actual = normalize_frame(frame, profile or expected.profile)
    if actual != expected:
        raise AssertionError(
            f"normalized {actual.profile} frame differs\nexpected={expected.to_dict()!r}\nactual={actual.to_dict()!r}"
        )
    return actual


def assert_snapshot_file(
    frame: ScreenFrame,
    path: str | Path,
    *,
    profile: SnapshotProfile = "spans",
    update: bool = False,
) -> FrameSnapshot:
    """Compare normalized evidence; update only under an explicit caller policy."""
    snapshot = normalize_frame(frame, profile)
    target = Path(path)
    encoded = json.dumps(snapshot.to_dict(), ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    if update:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(encoded, encoding="utf-8")
        return snapshot
    if not target.exists():
        raise AssertionError(f"snapshot does not exist: {target}")
    expected = target.read_text(encoding="utf-8")
    if expected != encoded:
        diff = _snapshot_diff(expected, encoded, target)
        raise AssertionError(f"terminal snapshot differs: {target}\n{diff}")
    return snapshot


def _snapshot_diff(expected: str, actual: str, target: Path) -> str:
    expected_lines: list[str] = expected.splitlines()
    actual_lines: list[str] = actual.splitlines()
    common_length = min(len(expected_lines), len(actual_lines))
    first_difference = next(
        (index for index in range(common_length) if expected_lines[index] != actual_lines[index]),
        common_length,
    )
    start = max(0, first_difference - 3)
    end = first_difference + 4
    lines: list[str] = [
        f"--- {target}",
        "+++ presented-frame",
        f"@@ lines {start + 1}-{end} @@",
    ]
    for index in range(start, min(end, max(len(expected_lines), len(actual_lines)))):
        expected_line = expected_lines[index] if index < len(expected_lines) else "<missing>"
        actual_line = actual_lines[index] if index < len(actual_lines) else "<missing>"
        if expected_line == actual_line:
            lines.append(f" {expected_line}")
            continue
        lines.append(f"-{expected_line}")
        lines.append(f"+{actual_line}")
    if start > 0 or end < max(len(expected_lines), len(actual_lines)):
        lines.append("... snapshot diff context truncated ...")
    return "\n".join(lines)


def assert_cursor(
    frame: ScreenFrame,
    *,
    row: int | None = None,
    column: int | None = None,
    visible: bool | None = None,
) -> None:
    cursor = frame.cursor
    mismatches: list[str] = []
    if row is not None and cursor.row != row:
        mismatches.append(f"row expected {row}, got {cursor.row}")
    if column is not None and cursor.column != column:
        mismatches.append(f"column expected {column}, got {cursor.column}")
    if visible is not None and cursor.visible != visible:
        mismatches.append(f"visible expected {visible}, got {cursor.visible}")
    if mismatches:
        raise AssertionError("cursor mismatch: " + "; ".join(mismatches))


def regex(pattern: str, flags: int = 0) -> Pattern[str]:
    return re.compile(pattern, flags)


def assert_screen_text(subject: ScreenFrame | _ScreenSubject, expected: str | Pattern[str]) -> None:
    """Compatibility helper accepting a frame or a sync session with screen.frame()."""
    if isinstance(subject, ScreenFrame):
        assert_frame_text(subject, expected)
        return
    screen = subject.screen
    assert_frame_text(screen.frame(), expected)


def stable_terminal_snapshot(session: _ScreenSubject, *, stable_for: float = 0.25) -> FrameSnapshot:
    capture = session.screen.capture_stable(stable_for=stable_for)
    return normalize_frame(capture.frame, "spans")


def assert_terminal_snapshot(
    session: _ScreenSubject,
    path: str | Path,
    *,
    update: bool = False,
    stable_for: float = 0.25,
) -> FrameSnapshot:
    """Compare a snapshot using the exact actor-returned stable frame."""
    capture = session.screen.capture_stable(stable_for=stable_for)
    return assert_snapshot_file(capture.frame, path, update=update)


TerminalSnapshot = FrameSnapshot


__all__ = [
    "FrameSnapshot",
    "TerminalSnapshot",
    "assert_cursor",
    "assert_frame_text",
    "assert_match_style",
    "assert_match_text",
    "assert_matches",
    "assert_neutral_region",
    "assert_no_matches",
    "assert_no_wrapped_rows",
    "assert_normalized_frame",
    "assert_screen_text",
    "assert_snapshot_file",
    "assert_terminal_snapshot",
    "regex",
    "stable_terminal_snapshot",
]
