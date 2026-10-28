#!/usr/bin/env python3
"""Raw-mode fixture that reports bracketed paste, focus, wheel, and key arrivals."""

import os
import re
import sys
import termios
import time
import tty

EVENT = re.compile(
    rb"\x1b\[200~(?P<paste>.*?)\x1b\[201~"
    rb"|\x1b\[(?P<focus>[IO])"
    rb"|\x1b\[<(?P<button>\d+);(?P<column>\d+);(?P<row>\d+)(?P<final>[Mm])"
    rb"|(?P<other>.)",
    re.DOTALL,
)
WHEEL = {64: "up", 65: "down"}


def write(text: str) -> None:
    os.write(sys.stdout.fileno(), text.encode())


def describe(match: re.Match[bytes]) -> str:
    if match["paste"] is not None:
        return "PASTE=" + match["paste"].decode()
    if match["focus"] is not None:
        return "FOCUS=" + ("in" if match["focus"] == b"I" else "out")
    if match["button"] is not None:
        direction = WHEEL.get(int(match["button"]), "button" + match["button"].decode())
        return f"WHEEL={direction}@{match['column'].decode()},{match['row'].decode()}"
    return "KEY=" + match["other"].hex()


stdin = sys.stdin.fileno()
original = termios.tcgetattr(stdin)
started = time.monotonic()
try:
    tty.setraw(stdin)
    write("\x1b[?2004h\x1b[?1004h\x1b[?1000h\x1b[?1006hREPORTER_READY\r\n")
    pending = b""
    while True:
        chunk = os.read(stdin, 4096)
        if not chunk:
            break
        arrived_ms = int((time.monotonic() - started) * 1000)
        pending += chunk
        if pending.startswith(b"\x1b[200~") and b"\x1b[201~" not in pending:
            continue
        for match in EVENT.finditer(pending):
            write(f"{describe(match)} T={arrived_ms}\r\n")
        pending = b""
finally:
    termios.tcsetattr(stdin, termios.TCSADRAIN, original)
