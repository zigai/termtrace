"""Strict type checks of the public Python contracts from a consumer's view."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
TYPES = Path(__file__).resolve().parent / "types"
# Each negative fixture must be rejected on exactly these lines and nowhere else.
REJECTED_LINES = {
    "contracts": [6, 7, 8, 9],
    "screen": [8, 9, 10, 11, 12],
    "control": [6, 7, 8, 9, 10, 11, 12],
    "testing": [12, 13, 14, 15, 16],
    "evidence": [10, 11, 12, 13, 14],
    "diagnostics": [10, 11, 12, 13, 14, 15, 23, 24],
    "original": [6, 7, 8, 9, 10],
    "editing": [8, 9, 10, 11, 12, 13],
}


def mypy(path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "mypy", "--strict", "--no-incremental", str(path)],
        check=False,
        cwd=REPO,
        env=dict(os.environ, MYPYPATH=str(REPO / "crates/bindings-python/python")),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


@pytest.mark.parametrize("name", sorted(REJECTED_LINES))
def test_type_contract(name: str) -> None:
    positive = mypy(TYPES / f"{name}_positive.py")
    assert positive.returncode == 0, positive.stdout

    negative = mypy(TYPES / f"{name}_negative.py")
    assert negative.returncode == 1, negative.stdout
    rejected = sorted(
        int(line.split(":")[1])
        for line in negative.stdout.splitlines()
        if ": error:" in line
    )
    assert rejected == REJECTED_LINES[name], negative.stdout
