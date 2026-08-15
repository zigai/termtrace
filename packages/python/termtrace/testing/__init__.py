"""Runner-neutral assertions and test ownership adapters."""

from termtrace.testing.lifecycle import (
    EvidencePolicy,
    FinalizationReport,
    LifecycleState,
    TermtraceTestConfig,
    TestOutcome,
    classify_outcome,
    create_evidence_directory,
)
from termtrace.testing.sync import TerminalAssertions, TermtraceTest

__all__ = [
    "EvidencePolicy",
    "FinalizationReport",
    "LifecycleState",
    "TerminalAssertions",
    "TermtraceTest",
    "TermtraceTestConfig",
    "TestOutcome",
    "classify_outcome",
    "create_evidence_directory",
]
