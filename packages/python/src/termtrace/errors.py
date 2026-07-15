"""Stable public error contracts."""


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

    def __init__(
        self,
        message: str,
        *,
        code: str,
        retry_safety: str,
        uncertain: bool,
        evidence: str | None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.retry_safety = retry_safety
        self.uncertain = uncertain
        self.evidence = evidence
