#!/usr/bin/env python3
"""Small alternate-screen fixture with mode-aware key input."""

import os
import sys
import termios
import tty


def write(data: bytes) -> None:
    os.write(sys.stdout.fileno(), data)


write(b"\x1b[?1049h\x1b[2J\x1b[HALTERNATE_READY\r\nPress an arrow or q")
stdin = sys.stdin.fileno()
original = termios.tcgetattr(stdin)
try:
    tty.setraw(stdin)
    value = os.read(stdin, 8)
    write(b"\r\nINPUT_HEX=" + value.hex().encode("ascii"))
finally:
    termios.tcsetattr(stdin, termios.TCSADRAIN, original)
    write(b"\x1b[?1049l")
