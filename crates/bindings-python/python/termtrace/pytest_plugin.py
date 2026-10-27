"""Truthful pytest outcomes with shared Rust owned-session finalization.

Finalize after teardown outcomes are known. Skips and xfails retain conservatively.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from typing import Any

import pytest

from termtrace import Recording, Termtrace
from termtrace.testing import TerminalTestContext

_contexts: pytest.StashKey[list[TerminalTestContext]] = pytest.StashKey()
_reports: pytest.StashKey[list[Any]] = pytest.StashKey()
_root: pytest.StashKey[str] = pytest.StashKey()


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo[Any]) -> Any:
    report = yield
    reports = item.stash.setdefault(_reports, [])
    reports.append(report)
    if report.when != "teardown":
        return report
    passed = all(r.passed and not hasattr(r, "wasxfail") for r in reports)
    for context in item.stash.get(_contexts, []):
        try:
            result = context.finalize("passed" if passed else "error")
            if result["retained"] or result["secondary_failures"]:
                report.sections.append(("Termtrace finalization", str(result)))
            failures = result["secondary_failures"]
        except BaseException as error:  # noqa: BLE001 - report teardown without replacing call/setup failure
            failures = [
                {"phase": "finalize", "message": str(error), "truncated": False}
            ]
            report.sections.append(
                (
                    "Termtrace finalization",
                    f"{failures}; evidence: {context.evidence_root}",
                )
            )
        if failures and report.passed:
            report.outcome = "failed"
            report.longrepr = f"Termtrace teardown failed: {failures}"
    return report


@pytest.fixture
def termtrace(tmp_path_factory: Any, request: pytest.FixtureRequest) -> Termtrace:
    """Provide an isolated client; only termtrace_session launches are helper-owned."""
    root = str(tmp_path_factory.mktemp("termtrace-evidence"))
    request.node.stash[_root] = root
    return Termtrace(binary=os.environ.get("TERMTRACE_BIN"), evidence_root=root)


@pytest.fixture
def termtrace_session(
    termtrace: Termtrace, request: pytest.FixtureRequest
) -> Callable[..., Recording]:
    """Keep the existing callable fixture; finalize after all pytest teardown outcomes exist."""
    context = TerminalTestContext(
        _client=termtrace, _owned_root=request.node.stash[_root]
    )
    request.node.stash.setdefault(_contexts, []).append(context)
    return context.record
