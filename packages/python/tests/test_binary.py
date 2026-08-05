from pathlib import Path

import pytest
from termtrace.binary import resolve_binary


def test_explicit_binary_override_is_resolved(tmp_path: Path) -> None:
    binary = tmp_path / "termtrace"
    binary.write_bytes(b"#!/bin/sh\nexit 0\n")
    binary.chmod(0o700)
    assert resolve_binary(binary) == binary.resolve()


def test_missing_explicit_binary_has_actionable_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("TERMTRACE_BINARY", raising=False)
    with pytest.raises(FileNotFoundError, match="TERMTRACE_BINARY"):
        resolve_binary(tmp_path / "missing")
