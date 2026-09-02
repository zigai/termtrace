"""Optional pytest ownership, assertions, evidence, and snapshot integration."""

from __future__ import annotations

import hashlib
import importlib
import os
import shlex
import shutil
import uuid
from collections.abc import AsyncIterator, Awaitable, Generator, Iterator, Sequence
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Any, Protocol

if TYPE_CHECKING:
    import pytest
    import pytest_asyncio
    from _pytest.reports import TestReport
    from pluggy import Result
else:
    pytest = importlib.import_module("pytest")
    try:
        pytest_asyncio = importlib.import_module("pytest_asyncio")
    except ModuleNotFoundError:
        pytest_asyncio = None

from termtrace.environment import EnvironmentOptions
from termtrace.testing.async_ import AsyncTermtraceTest
from termtrace.testing.lifecycle import (
    EvidencePolicy,
    FinalizationReport,
    TermtraceTestConfig,
    TestOutcome,
    classify_outcome,
)
from termtrace.testing.sync import TermtraceTest


class SyncTestFactory(Protocol):
    def __call__(
        self,
        command: Sequence[str | Path],
        *,
        evidence_policy: EvidencePolicy | None = None,
        cols: int | None = None,
        rows: int | None = None,
        working_directory: str | Path | None = None,
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
        binary_path: str | Path | None = None,
    ) -> TermtraceTest: ...


class AsyncTestFactory(Protocol):
    def __call__(
        self,
        command: Sequence[str | Path],
        *,
        evidence_policy: EvidencePolicy | None = None,
        cols: int | None = None,
        rows: int | None = None,
        working_directory: str | Path | None = None,
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
        binary_path: str | Path | None = None,
    ) -> Awaitable[AsyncTermtraceTest]: ...


def pytest_addoption(parser: Any) -> None:
    group = parser.getgroup("termtrace")
    group.addoption(
        "--termtrace-update-snapshots",
        action="store_true",
        default=False,
        help="explicitly update deterministic terminal snapshots",
    )
    group.addoption(
        "--termtrace-evidence-root",
        action="store",
        default=None,
        help="root for isolated per-test Termtrace evidence",
    )
    group.addoption(
        "--termtrace-evidence",
        choices=("on_failure", "always", "manual"),
        default=None,
        help="Termtrace test evidence retention policy",
    )
    group.addoption(
        "--termtrace-finalization-timeout",
        type=float,
        default=None,
        help="bounded seconds for each supported finalization phase",
    )
    parser.addini(
        "termtrace_evidence_root",
        "root for isolated per-test Termtrace evidence",
        default=".termtrace-evidence",
    )
    parser.addini(
        "termtrace_evidence",
        "evidence retention: on_failure, always, or manual",
        default="on_failure",
    )
    parser.addini(
        "termtrace_finalization_timeout",
        "seconds for supported finalization phases",
        default="10",
    )


def pytest_configure(config: Any) -> None:
    config.addinivalue_line(
        "markers",
        "termtrace(command, **options): configure the owned termtrace_test fixture",
    )
    config._termtrace_run_id = uuid.uuid4().hex


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item: Any, call: Any) -> Generator[None, Result[TestReport], None]:
    outcome: Result[TestReport] = yield
    report = outcome.get_result()
    reports = getattr(item, "_termtrace_reports", None)
    if reports is None:
        reports = {}
        item._termtrace_reports = reports
    reports[report.when] = report
    if report.when == "call" and call.excinfo is not None:
        item._termtrace_call_outcome = classify_outcome(call.excinfo.value)
    if report.when == "teardown" and report.failed:
        finalized: list[FinalizationReport] = getattr(item, "_termtrace_finalization_reports", [])
        evidence: list[str] = []
        for finalization in finalized[:16]:
            if finalization.evidence_directory is not None:
                evidence.append(f"Termtrace evidence: {str(finalization.evidence_directory)[:512]}")
            evidence.extend(
                f"Verified artifact: {str(path)[:512]}"
                for path in finalization.verified_artifacts[:4]
            )
        if evidence:
            report.sections.append(("Termtrace evidence", "\n".join(evidence)))


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_teardown(item: Any, nextitem: Any) -> Generator[None, Result[None], None]:
    outcome: Result[None] = yield
    if outcome.excinfo is not None or _pytest_item_outcome(item) != "passed":
        return
    deferred: list[Path] = getattr(item, "_termtrace_deferred_discards", [])
    for directory in deferred:
        if directory.exists():
            shutil.rmtree(directory)


@pytest.fixture
def termtrace_test_factory(request: Any) -> Iterator[SyncTestFactory]:
    owners: list[TermtraceTest] = []
    base = _base_config(request)

    def factory(
        command: Sequence[str | Path],
        *,
        evidence_policy: EvidencePolicy | None = None,
        cols: int | None = None,
        rows: int | None = None,
        working_directory: str | Path | None = None,
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
        binary_path: str | Path | None = None,
    ) -> TermtraceTest:
        selected_policy = evidence_policy or base.evidence_policy
        config = replace(
            base,
            command=tuple(command),
            evidence_policy="always" if selected_policy == "on_failure" else selected_policy,
            cols=cols if cols is not None else base.cols,
            rows=rows if rows is not None else base.rows,
            working_directory=working_directory,
            environment=environment,
            output_byte_budget=output_byte_budget,
            binary_path=binary_path,
            attempt=base.attempt + len(owners),
        )
        owner = TermtraceTest.start(config)
        owners.append(owner)
        if selected_policy == "on_failure":
            _defer_evidence_discard(request, owner.evidence_directory)
        return owner

    yield factory
    _finalize_sync_owners(request, owners)


@pytest.fixture
def termtrace_test(request: Any, termtrace_test_factory: SyncTestFactory) -> TermtraceTest:
    command, options = _marker_launch(request)
    return termtrace_test_factory(command, **options)


if pytest_asyncio is not None:

    @pytest_asyncio.fixture
    async def async_termtrace_test_factory(
        request: Any,
    ) -> AsyncIterator[AsyncTestFactory]:
        owners: list[AsyncTermtraceTest] = []
        base = replace(_base_config(request), adapter="pytest-asyncio")

        async def factory(
            command: Sequence[str | Path],
            *,
            evidence_policy: EvidencePolicy | None = None,
            cols: int | None = None,
            rows: int | None = None,
            working_directory: str | Path | None = None,
            environment: EnvironmentOptions | None = None,
            output_byte_budget: int | None = None,
            binary_path: str | Path | None = None,
        ) -> AsyncTermtraceTest:
            selected_policy = evidence_policy or base.evidence_policy
            config = replace(
                base,
                command=tuple(command),
                evidence_policy="always" if selected_policy == "on_failure" else selected_policy,
                cols=cols if cols is not None else base.cols,
                rows=rows if rows is not None else base.rows,
                working_directory=working_directory,
                environment=environment,
                output_byte_budget=output_byte_budget,
                binary_path=binary_path,
                attempt=base.attempt + len(owners),
            )
            owner = await AsyncTermtraceTest.start(config)
            owners.append(owner)
            if selected_policy == "on_failure":
                _defer_evidence_discard(request, owner.evidence_directory)
            return owner

        yield factory
        await _finalize_async_owners(request, owners)

    @pytest_asyncio.fixture
    async def async_termtrace_test(
        request: Any,
        async_termtrace_test_factory: AsyncTestFactory,
    ) -> AsyncTermtraceTest:
        command, options = _marker_launch(request)
        return await async_termtrace_test_factory(command, **options)

else:

    @pytest.fixture
    def async_termtrace_test_factory() -> None:
        pytest.fail("async_termtrace_test_factory requires the 'termtrace[pytest-async]' extra")

    @pytest.fixture
    def async_termtrace_test() -> None:
        pytest.fail("async_termtrace_test requires the 'termtrace[pytest-async]' extra")


def _base_config(request: Any) -> TermtraceTestConfig:
    config = request.config
    evidence_root_value = config.getoption("--termtrace-evidence-root")
    if evidence_root_value is None:
        evidence_root_value = config.getini("termtrace_evidence_root")
    evidence_value = config.getoption("--termtrace-evidence")
    if evidence_value is None:
        evidence_value = config.getini("termtrace_evidence")
    if evidence_value not in {"on_failure", "always", "manual"}:
        raise pytest.UsageError("termtrace evidence policy must be on_failure, always, or manual")
    timeout_value = config.getoption("--termtrace-finalization-timeout")
    if timeout_value is None:
        timeout_value = float(config.getini("termtrace_finalization_timeout"))
    if timeout_value <= 0:
        raise pytest.UsageError("termtrace finalization timeout must be positive")
    root = Path(config.rootpath) / Path(evidence_root_value)
    snapshot_root = (
        Path(str(request.path)).parent
        / "__snapshots__"
        / hashlib.sha256(request.node.nodeid.encode()).hexdigest()[:24]
    )
    return TermtraceTestConfig(
        command=(),
        evidence_root=root,
        snapshot_root=snapshot_root,
        evidence_policy=evidence_value,
        update_snapshots=bool(config.getoption("--termtrace-update-snapshots")),
        finalization_timeout=timeout_value,
        run_id=config._termtrace_run_id,
        worker_id=os.environ.get("PYTEST_XDIST_WORKER", "main"),
        test_id=request.node.nodeid,
        attempt=max(int(getattr(request.node, "execution_count", 1)) - 1, 0),
    )


def _marker_launch(request: Any) -> tuple[tuple[str | Path, ...], dict[str, Any]]:
    marker = request.node.get_closest_marker("termtrace")
    if marker is None:
        raise pytest.UsageError(
            "termtrace_test requires @pytest.mark.termtrace(command=[...]); "
            "use termtrace_test_factory for inline launch options"
        )
    command_value = marker.kwargs.get("command")
    if command_value is None and marker.args:
        command_value = marker.args[0]
    if isinstance(command_value, str):
        command: tuple[str | Path, ...] = tuple(shlex.split(command_value))
    elif isinstance(command_value, Sequence):
        command = tuple(command_value)
    else:
        raise pytest.UsageError("termtrace marker command must be a string or sequence")
    options = {key: value for key, value in marker.kwargs.items() if key != "command"}
    return command, options


def _pytest_item_outcome(item: Any) -> TestOutcome:
    reports = getattr(item, "_termtrace_reports", {})
    teardown_report = reports.get("teardown")
    if teardown_report is not None and teardown_report.failed:
        return "error"
    call_report = reports.get("call")
    if call_report is not None:
        if call_report.passed or getattr(call_report, "wasxfail", False):
            return "passed"
        if call_report.failed:
            return getattr(item, "_termtrace_call_outcome", "error")
        return "error"
    setup_report = reports.get("setup")
    if setup_report is not None and setup_report.failed:
        return "error"
    return "error"


def _pytest_outcome(request: Any) -> TestOutcome:
    return _pytest_item_outcome(request.node)


def _defer_evidence_discard(request: Any, directory: Path) -> None:
    deferred: list[Path] | None = getattr(request.node, "_termtrace_deferred_discards", None)
    if deferred is None:
        deferred = []
        request.node._termtrace_deferred_discards = deferred
    deferred.append(directory)


def _store_finalization_reports(request: Any, reports: list[FinalizationReport]) -> None:
    finalized: list[FinalizationReport] | None = getattr(
        request.node, "_termtrace_finalization_reports", None
    )
    if finalized is None:
        finalized = []
        request.node._termtrace_finalization_reports = finalized
    finalized.extend(reports)


def _finalize_sync_owners(request: Any, owners: list[TermtraceTest]) -> None:
    outcome = _pytest_outcome(request)
    reports: list[FinalizationReport] = []
    for owner in reversed(owners):
        reports.append(owner.finalize(outcome=outcome))
    _store_finalization_reports(request, reports)
    _report_finalization(request, outcome, reports)


async def _finalize_async_owners(
    request: Any,
    owners: list[AsyncTermtraceTest],
) -> None:
    outcome = _pytest_outcome(request)
    reports: list[FinalizationReport] = []
    for owner in reversed(owners):
        reports.append(await owner.finalize(outcome=outcome))
    _store_finalization_reports(request, reports)
    _report_finalization(request, outcome, reports)


def _report_finalization(
    request: Any,
    outcome: TestOutcome,
    reports: list[FinalizationReport],
) -> None:
    if outcome != "passed":
        evidence: list[str] = []
        for report in reports[:16]:
            if report.evidence_directory is None:
                continue
            evidence.append(f"Termtrace evidence: {str(report.evidence_directory)[:512]}")
            evidence.extend(
                f"Verified artifact: {str(path)[:512]}" for path in report.verified_artifacts[:4]
            )
        if evidence:
            request.node.add_report_section("teardown", "termtrace evidence", "\n".join(evidence))

    failures: list[str] = [message for report in reports for message in report.secondary_failures]
    if not failures:
        return
    detail = "\n".join(f"- {message}" for message in failures)
    request.node.add_report_section("teardown", "termtrace", detail)
    if outcome == "passed":
        raise RuntimeError(f"Termtrace finalization failed:\n{detail}")
