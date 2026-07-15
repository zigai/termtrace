"""Typed clients for Termtrace canonical terminal evidence."""

from termtrace.client import (
    DriverTransport,
    Evidence,
    Keyboard,
    Output,
    Screen,
    Session,
    Terminal,
)
from termtrace.async_client import (
    AsyncEvidence,
    AsyncKeyboard,
    AsyncOutput,
    AsyncScreen,
    AsyncSession,
    AsyncTerminal,
)
from termtrace.errors import (
    TermtraceError,
    TermtraceOperationError,
    TermtraceProtocolError,
)

__all__ = [
    "DriverTransport",
    "AsyncEvidence",
    "AsyncKeyboard",
    "AsyncOutput",
    "AsyncScreen",
    "AsyncSession",
    "AsyncTerminal",
    "Evidence",
    "Keyboard",
    "Output",
    "Screen",
    "Session",
    "Terminal",
    "TermtraceError",
    "TermtraceOperationError",
    "TermtraceProtocolError",
]
