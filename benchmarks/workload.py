#!/usr/bin/env python3
"""Interactive query-heavy PTY workload; bulk growth retains the real recorder path."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import select
import sys
import termios
import tty


def emit(data: bytes) -> None:
    remaining = memoryview(data)
    while remaining:
        remaining = remaining[os.write(sys.stdout.fileno(), remaining) :]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--control", type=Path, required=True)
    args = parser.parse_args()
    original = termios.tcgetattr(sys.stdin.fileno())
    tty.setraw(sys.stdin.fileno())
    control = os.open(args.control, os.O_RDWR | os.O_NONBLOCK)
    pending = b""
    sequence = 0
    try:
        emit(b"\x1b[?1049h\x1b[2J\x1b[HREADY\x1b[6n")
        while True:
            ready, _, _ = select.select([control, sys.stdin.fileno()], [], [])
            if sys.stdin.fileno() in ready:
                data = os.read(sys.stdin.fileno(), 65536)
                if not data:
                    return
                # Query responses terminate in R; benchmark input always ends in newline.
                if b"\n" in data:
                    sequence += 1
                    emit(f"\x1b[2J\x1b[HREADY INPUT-{sequence}\n".encode())
            if control in ready:
                pending += os.read(control, 4096)
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    count = int(line)
                    if count == 0:
                        return
                    for _ in range(count):
                        emit(b"\x1b[H" + b"x" * 4089 + b"\x1b[6n")
                        reply = b""
                        while not reply.endswith(b"R"):
                            readable, _, _ = select.select(
                                [sys.stdin.fileno()], [], [], 30
                            )
                            if not readable:
                                raise TimeoutError(
                                    "recorder did not answer cursor query"
                                )
                            reply += os.read(sys.stdin.fileno(), 4096)
                    emit(b"\x1b[2J\x1b[HREADY GROWN\n")
                    args.control.with_suffix(".ack").write_text(
                        str(count), encoding="ascii"
                    )
    finally:
        os.close(control)
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, original)


if __name__ == "__main__":
    main()
