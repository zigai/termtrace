#!/usr/bin/env python3
"""Compile-positive and compile-negative external Python contracts, without ignores."""

import os
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
env = dict(os.environ, MYPYPATH=str(root / "crates/bindings-python/python"))
types = Path("tests/consumers/python_native/types")
# Each negative fixture must be rejected on exactly these lines and nowhere else.
fixtures = {
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
        [sys.executable, "-m", "mypy", "--strict", str(path)],
        check=False,
        cwd=root,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


for name, lines in fixtures.items():
    positive = mypy(types / f"{name}_positive.py")
    negative = mypy(types / f"{name}_negative.py")
    print(positive.stdout, negative.stdout, sep="\n")
    if positive.returncode or negative.returncode != 1:
        sys.exit(1)
    for line in lines:
        if f"{name}_negative.py:{line}: error:" not in negative.stdout:
            sys.exit(f"missing expected type rejection in {name} on line {line}")
    if negative.stdout.count(": error:") != len(lines):
        sys.exit(f"unexpected type rejections in {name}_negative.py")
