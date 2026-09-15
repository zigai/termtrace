"""Pytest fixtures for Termtrace terminal recording."""

from __future__ import annotations

import os
from collections.abc import Callable, Generator
from typing import Any

import pytest

from termtrace import Recording, Termtrace


@pytest.fixture
def termtrace(tmp_path_factory: Any) -> Termtrace:
    """Provide an isolated Termtrace client for the test."""
    evidence_root = str(tmp_path_factory.mktemp("termtrace-evidence"))
    bin_override = os.environ.get("TERMTRACE_BIN")
    return Termtrace(binary=bin_override, evidence_root=evidence_root)


@pytest.fixture
def termtrace_session(termtrace: Termtrace) -> Generator[Callable[..., Recording], None, None]:
    """Provide a helper to launch and automatically clean up recordings."""
    active_recordings: list[Recording] = []

    def _launch(command: list[str], **kwargs: Any) -> Recording:
        rec = termtrace.record(command, **kwargs)
        active_recordings.append(rec)
        return rec

    yield _launch

    for rec in active_recordings:
        if rec.is_active():
            rec.stop()
