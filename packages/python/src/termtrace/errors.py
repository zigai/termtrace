"""Stable public error contracts."""

from termtrace._generated.driver_response import ControlErrorDetails


class TermtraceError(RuntimeError):
    """Base class for operational client failures."""


class TermtraceProtocolError(TermtraceError):
    """The driver emitted an invalid or uncorrelated response."""


class TermtraceOperationError(TermtraceError):
    """A typed driver operation was rejected."""

    code: str
    retry_safety: str
    uncertain: bool
    evidence: str | None
    details: ControlErrorDetails | None

    def __init__(
        self,
        message: str,
        *,
        code: str,
        retry_safety: str,
        uncertain: bool,
        evidence: str | None,
        details: ControlErrorDetails | None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.retry_safety = retry_safety
        self.uncertain = uncertain
        self.evidence = evidence
        self.details = details
