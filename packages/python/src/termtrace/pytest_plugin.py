"""Optional pytest assertions and explicit terminal-snapshot updates."""

import importlib
from pathlib import Path
from typing import Any

pytest: Any = importlib.import_module("pytest")

from termtrace.assertions import (
    TerminalSnapshot,
    assert_cursor,
    assert_no_presented_transient,
    assert_screen_text,
    assert_terminal_snapshot,
)
from termtrace.client import Session


class TermtraceAssertions:
    """Per-test assertion facade with one explicit snapshot-update policy."""

    def __init__(self, *, update_snapshots: bool) -> None:
        self.update_snapshots = update_snapshots

    def screen_text(self, session: Session, expected: str) -> None:
        assert_screen_text(session, expected)

    def cursor(
        self,
        session: Session,
        *,
        row: int | None = None,
        column: int | None = None,
        visible: bool | None = None,
    ) -> None:
        assert_cursor(session, row=row, column=column, visible=visible)

    def no_presented_transient(self, session: Session) -> None:
        assert_no_presented_transient(session)

    def snapshot(
        self, session: Session, path: str | Path, *, stable_for: float = 0.25
    ) -> TerminalSnapshot:
        return assert_terminal_snapshot(
            session,
            path,
            update=self.update_snapshots,
            stable_for=stable_for,
        )


def pytest_addoption(parser: Any) -> None:
    parser.addoption(
        "--termtrace-update-snapshots",
        action="store_true",
        default=False,
        help="explicitly update stable normalized terminal snapshots",
    )


@pytest.fixture  # type: ignore[untyped-decorator]
def termtrace_assertions(request: Any) -> TermtraceAssertions:
    """Provide evidence-located screen, cursor, transient, and snapshot assertions."""

    return TermtraceAssertions(
        update_snapshots=bool(request.config.getoption("--termtrace-update-snapshots"))
    )
