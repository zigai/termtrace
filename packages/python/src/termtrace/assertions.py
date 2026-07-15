"""Assertion-library-neutral semantic screen and snapshot helpers."""

import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Pattern

from termtrace.client import Session
from termtrace.models import JsonValue


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
    modes: JsonValue


def stable_terminal_snapshot(session: Session, *, stable_for: float = 0.25) -> TerminalSnapshot:
    """Prove presented-state stability and return a normalized frame."""

    observed = _mapping(session.semantic.inspect(), "observation")
    state_hash = _string(observed, "state_hash")
    if observed.get("source_completed") is not True:
        session.screen.wait_for(
            {
                "predicate": "state_hash",
                "hash": state_hash,
                "comparison": "unchanged",
            },
            stable_for=stable_for,
        )
    return _normalize_frame(session.screen.frame())


def assert_screen_text(session: Session, expected: str | Pattern[str]) -> None:
    """Assert against the current presented viewport, not output history."""

    text = session.screen.text()
    matched = expected in text if isinstance(expected, str) else expected.search(text) is not None
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
    expected = {"cursor_row": row, "cursor_column": column, "cursor_visible": visible}
    for field, value in expected.items():
        if value is not None and frame.get(field) != value:
            raise AssertionError(
                f"{field} expected {value!r}, received {frame.get(field)!r}\n"
                f"evidence: {session.canonical_session}"
            )


def assert_no_presented_transient(session: Session) -> None:
    """Reject evidence-backed presentation-eligible transient findings."""

    report = _mapping(session.semantic.diagnose(("transient",)), "report")
    findings = report.get("findings")
    if not isinstance(findings, list):
        raise AssertionError("diagnostic report is missing findings")
    if any(isinstance(item, dict) and item.get("code") == "presented_transient" for item in findings):
        raise AssertionError(f"presented transient found\nevidence: {session.canonical_session}")


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
    encoded = json.dumps(asdict(snapshot), ensure_ascii=False, indent=2, sort_keys=True) + "\n"
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


def _normalize_frame(frame: dict[str, JsonValue]) -> TerminalSnapshot:
    modes = frame.get("modes")
    return TerminalSnapshot(
        version=1,
        cols=_integer(frame, "cols"),
        rows=_integer(frame, "rows"),
        viewport_text=_string(frame, "viewport_text"),
        cursor=CursorSnapshot(
            column=_optional_integer(frame, "cursor_column"),
            row=_optional_integer(frame, "cursor_row"),
            visible=_boolean(frame, "cursor_visible"),
        ),
        modes=modes,
    )


def _mapping(value: dict[str, JsonValue], field: str) -> dict[str, JsonValue]:
    item = value.get(field)
    if not isinstance(item, dict):
        raise AssertionError(f"{field} must be an object")
    return item


def _string(value: dict[str, JsonValue], field: str) -> str:
    item = value.get(field)
    if not isinstance(item, str):
        raise AssertionError(f"{field} must be a string")
    return item


def _integer(value: dict[str, JsonValue], field: str) -> int:
    item = value.get(field)
    if not isinstance(item, int) or isinstance(item, bool):
        raise AssertionError(f"{field} must be an integer")
    return item


def _optional_integer(value: dict[str, JsonValue], field: str) -> int | None:
    if value.get(field) is None:
        return None
    return _integer(value, field)


def _boolean(value: dict[str, JsonValue], field: str) -> bool:
    item = value.get(field)
    if not isinstance(item, bool):
        raise AssertionError(f"{field} must be a boolean")
    return item
