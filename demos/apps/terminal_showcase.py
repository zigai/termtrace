#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import select
import signal
import sys
import termios
import time
import tty
from enum import StrEnum
from types import FrameType


ESC = b"\x1b"
CSI = ESC + b"["
ST = ESC + b"\\"


class DemoMode(StrEnum):
    SEMANTIC = "semantic"
    CONTROL = "control"


class TerminalShowcase:
    def __init__(self, mode: DemoMode) -> None:
        self.mode = mode
        self.input_fd = sys.stdin.fileno()
        self.original_termios = termios.tcgetattr(self.input_fd)
        self.seen_signals: set[int] = set()
        self.should_terminate = False

    def run(self) -> None:
        tty.setraw(self.input_fd, when=termios.TCSANOW)
        self._install_signal_handlers()
        try:
            if self.mode is DemoMode.SEMANTIC:
                self._run_semantic_showcase()
            else:
                self._run_control_showcase()
        finally:
            termios.tcsetattr(self.input_fd, termios.TCSANOW, self.original_termios)

    def _install_signal_handlers(self) -> None:
        signal.signal(signal.SIGWINCH, self._handle_resize)
        for number in (
            signal.SIGINT,
            signal.SIGHUP,
            signal.SIGUSR1,
            signal.SIGUSR2,
            signal.SIGTERM,
        ):
            signal.signal(number, self._handle_signal)

    def _handle_resize(self, _number: int, _frame: FrameType | None) -> None:
        size = os.get_terminal_size(self.input_fd)
        self._line(f"RESIZED:{size.columns}x{size.lines}", color="38;5;45")

    def _handle_signal(self, number: int, _frame: FrameType | None) -> None:
        self.seen_signals.add(number)
        name = signal.Signals(number).name
        self._line(f"SIGNAL:{name}", color="38;5;214")
        if number == signal.SIGTERM:
            self.should_terminate = True

    def _run_semantic_showcase(self) -> None:
        self._emit(CSI + b"2J" + CSI + b"H" + ESC + b"]0;Termtrace semantic demo" + ST)
        self._line("TERMTRACE / VISUAL + SEMANTIC SHOWCASE", color="1;38;2;102;204;255")
        self._line("Unicode graphemes: 界  café  e\u0301  🚀", color="38;2;242;242;242")
        self._line("Truecolor: red / green / blue", color="38;2;255;80;80")

        self._line("Synchronized output transaction:", color="38;5;141")
        self._emit(CSI + b"?2026h")
        for step in range(1, 5):
            self._emit(f"\r  staged update {step}/4".encode())
            time.sleep(0.18)
        self._emit(CSI + b"?2026l" + b"\r\n")

        self._emit(CSI + b"?1049h" + CSI + b"H")
        self._line("┌──────────────────────────────────────────┐", color="38;5;45")
        self._line("│ Alternate-screen dashboard               │", color="38;5;45")
        self._line("└──────────────────────────────────────────┘", color="38;5;45")
        for percent in (20, 40, 60, 80, 100):
            self._emit(
                CSI
                + b"5;1H"
                + f"Progress [{percent:3}%] {'█' * (percent // 10):<10}".encode()
            )
            time.sleep(0.22)
        time.sleep(0.35)
        self._emit(CSI + b"?1049l")

        color_reply = self._terminal_query(ESC + b"]11;?" + ST, ST)
        cursor_reply = self._terminal_query(CSI + b"6n", b"R")
        self._line(f"BACKGROUND_QUERY:{color_reply.hex()}", color="38;5;111")
        self._line(f"CURSOR_QUERY:{cursor_reply.hex()}", color="38;5;111")
        self._line("READY_FOR_AGENT", color="1;38;5;82")

        payload = self._read_until(b"\n", timeout_seconds=15.0).rstrip(b"\r\n")
        self._line(f"AGENT_INPUT:{payload.decode(errors='replace')}", color="38;5;82")
        self._line("WAITING_FOR_USER1", color="38;5;214")
        self._wait_for_signal(signal.SIGUSR1, timeout_seconds=15.0)
        self._line("SEMANTIC_DEMO_COMPLETE", color="1;38;2;102;255;102")
        self._line(
            "Raw events → libghostty states → real Ghostty pixels", color="38;5;250"
        )
        time.sleep(0.9)

    def _run_control_showcase(self) -> None:
        self._emit(CSI + b"2J" + CSI + b"H" + ESC + b"]0;Termtrace control demo" + ST)
        self._line("TERMTRACE / LIVE CONTROL TIMELINE", color="1;38;2;102;204;255")
        self._line("CONTROL_READY", color="1;38;5;82")
        deadline = time.monotonic() + 25.0
        pending = bytearray()
        while not self.should_terminate and time.monotonic() < deadline:
            readable, _, _ = select.select([self.input_fd], [], [], 0.1)
            if not readable:
                continue
            pending.extend(os.read(self.input_fd, 4096))
            while b"\n" in pending:
                line, _, remainder = pending.partition(b"\n")
                pending = bytearray(remainder)
                text = line.rstrip(b"\r").decode(errors="replace")
                self._line(f"INPUT:{text}", color="38;5;82")
        if not self.should_terminate:
            raise TimeoutError("control demo did not receive graceful stop")
        self._line("CONTROL_DEMO_STOPPED_GRACEFULLY", color="1;38;5;82")
        time.sleep(0.5)

    def _terminal_query(self, query: bytes, terminator: bytes) -> bytes:
        self._emit(query)
        return self._read_until(terminator, timeout_seconds=5.0)

    def _read_until(self, terminator: bytes, timeout_seconds: float) -> bytes:
        deadline = time.monotonic() + timeout_seconds
        received = bytearray()
        while not received.endswith(terminator):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"terminal input did not end with {terminator!r}")
            readable, _, _ = select.select([self.input_fd], [], [], remaining)
            if not readable:
                continue
            received.extend(os.read(self.input_fd, 4096))
        return bytes(received)

    def _wait_for_signal(self, number: int, timeout_seconds: float) -> None:
        deadline = time.monotonic() + timeout_seconds
        while number not in self.seen_signals:
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"signal {signal.Signals(number).name} was not received"
                )
            time.sleep(0.05)

    @staticmethod
    def _emit(data: bytes) -> None:
        os.write(sys.stdout.fileno(), data)

    def _line(self, text: str, color: str) -> None:
        self._emit(f"\x1b[{color}m{text}\x1b[0m\r\n".encode())


def parse_mode() -> DemoMode:
    parser = argparse.ArgumentParser(
        description="Deterministic interactive Termtrace demo target"
    )
    parser.add_argument("mode", choices=list(DemoMode))
    arguments = parser.parse_args()
    return DemoMode(arguments.mode)


def main() -> None:
    TerminalShowcase(parse_mode()).run()


if __name__ == "__main__":
    main()
