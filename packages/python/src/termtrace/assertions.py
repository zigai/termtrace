"""Assertion-library-neutral semantic screen and snapshot helpers."""

import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Pattern, cast

import msgspec

from termtrace.client import Session
from termtrace.models import Frame, JsonValue


@dataclass(frozen=True, slots=True)
class CursorSnapshot:
    column: int | None
    row: int | None
    visible: bool


@dataclass(frozen=True, slots=True)
class TerminalSnapshot:
    """Stable normalized frame without volatile evidence identity."""

    version: int
    cols: int
    rows: int
    viewport_text: str
    cursor: CursorSnapshot
    modes: dict[str, JsonValue]


def stable_terminal_snapshot(
    session: Session, *, stable_for: float = 0.25
) -> TerminalSnapshot:
    """Prove presented-state stability and return a normalized frame."""

    observed = session.semantic.inspect()
    if not observed.source_completed:
        session.screen.wait_for(
            {
                "predicate": "state_hash",
                "hash": observed.state_hash,
                "comparison": "unchanged",
            },
            stable_for=stable_for,
        )
    return _normalize_frame(session.screen.frame())


def assert_screen_text(session: Session, expected: str | Pattern[str]) -> None:
    """Assert against the current presented viewport, not output history."""

    text = session.screen.text()
    matched = (
        expected in text
        if isinstance(expected, str)
        else expected.search(text) is not None
    )
    if not matched:
        raise AssertionError(
            f"presented screen did not match {expected!r}\n{text}\nevidence: {session.canonical_session}"
        )


def assert_cursor(
    session: Session,
    *,
    row: int | None = None,
    column: int | None = None,
    visible: bool | None = None,
) -> None:
    """Assert selected normalized cursor properties."""

    frame = session.screen.frame()
    actual = {
        "cursor_row": _optional_int(frame.cursor_row),
        "cursor_column": _optional_int(frame.cursor_column),
        "cursor_visible": frame.cursor_visible,
    }
    expected = {"cursor_row": row, "cursor_column": column, "cursor_visible": visible}
    for field, value in expected.items():
        if value is not None and actual[field] != value:
            raise AssertionError(
                f"{field} expected {value!r}, received {actual[field]!r}\n"
                f"evidence: {session.canonical_session}"
            )


def assert_no_presented_transient(session: Session) -> None:
    """Reject evidence-backed presentation-eligible transient findings."""

    report = session.semantic.diagnose(("transient",))
    if any(item.code == "presented_transient" for item in report.findings):
        raise AssertionError(
            f"presented transient found\nevidence: {session.canonical_session}"
        )


def assert_terminal_snapshot(
    session: Session,
    path: str | Path,
    *,
    update: bool = False,
    stable_for: float = 0.25,
) -> TerminalSnapshot:
    """Compare or explicitly update one stable normalized JSON snapshot."""

    snapshot = stable_terminal_snapshot(session, stable_for=stable_for)
    destination = Path(path)
    encoded = (
        json.dumps(asdict(snapshot), ensure_ascii=False, indent=2, sort_keys=True)
        + "\n"
    )
    if update:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(encoded, encoding="utf-8")
        return snapshot
    if not destination.is_file():
        raise AssertionError(
            f"terminal snapshot is missing: {destination}; use explicit update=True to create it"
        )
    expected = destination.read_text(encoding="utf-8")
    if expected != encoded:
        raise AssertionError(
            f"terminal snapshot differs: {destination}\nexpected:\n{expected}\nactual:\n{encoded}"
            f"evidence: {session.canonical_session}"
        )
    return snapshot


def regex(pattern: str) -> Pattern[str]:
    """Compile a typed regular expression for screen assertions."""

    return re.compile(pattern)


def _normalize_frame(frame: Frame) -> TerminalSnapshot:
    modes = _json_mapping(msgspec.to_builtins(frame.modes))
    return TerminalSnapshot(
        version=1,
        cols=frame.cols,
        rows=frame.rows,
        viewport_text=frame.viewport_text,
        cursor=CursorSnapshot(
            column=_optional_int(frame.cursor_column),
            row=_optional_int(frame.cursor_row),
            visible=frame.cursor_visible,
        ),
        modes=modes,
    )


def _optional_int(value: int | None | msgspec.UnsetType) -> int | None:
    if value is None or value is msgspec.UNSET:
        return None
    return value


def _json_mapping(value: object) -> dict[str, JsonValue]:
    if not isinstance(value, dict):
        raise AssertionError("terminal modes must encode as a JSON object")
    # The runtime dict check establishes the container; Pyright cannot retain
    # concrete object parameters when narrowing from `object`.
    mapping = cast("dict[object, object]", value)
    result: dict[str, JsonValue] = {}
    for key, item in mapping.items():
        if not isinstance(key, str):
            raise AssertionError("terminal modes must encode with string keys")
        result[key] = _json_value(item)
    return result


def _json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, list):
        items = cast("list[object]", value)
        return [_json_value(item) for item in items]
    if isinstance(value, dict):
        return _json_mapping(cast("dict[object, object]", value))
    raise AssertionError("generated terminal model did not encode as JSON")
