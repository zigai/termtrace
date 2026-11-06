#!/usr/bin/env python3
"""Detached terminal fixture that requires audited host query responses."""

import os
import select
import sys
import termios
import time
import tty

stdout = sys.stdout.fileno()
stdin = sys.stdin.fileno()
original = termios.tcgetattr(stdin)
responses = bytearray()
try:
    tty.setraw(stdin)
    os.write(stdout, b"\x1b]11;?\x1b\\\x1b[18t\x1b[14t\x1b[6n")
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        readable, _, _ = select.select((stdin,), (), (), 0.05)
        if not readable:
            continue
        chunk = os.read(stdin, 4096)
        if not chunk:
            break
        responses.extend(chunk)
        if b"R" in responses and (b"rgb:" in responses or b"#" in responses):
            break
finally:
    termios.tcsetattr(stdin, termios.TCSADRAIN, original)
if not responses:
    raise SystemExit("terminal host returned no query responses")
os.write(stdout, b"QUERY_RESPONSES_OK\r\n")
