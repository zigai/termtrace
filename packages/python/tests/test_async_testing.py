from __future__ import annotations

import asyncio
import json
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

import termtrace.testing.lifecycle as testing_lifecycle
from termtrace.session.async_ import AsyncSession
from termtrace.testing.async_ import AsyncTermtraceTest
from termtrace.testing.lifecycle import (
    TermtraceTestConfig,
    _ArtifactHashBudget,
    create_evidence_directory,
)
from termtrace.transport.async_ import AsyncTerminal


class FakeAsyncScreen:
    def __init__(self, operations: list[str]) -> None:
        self.operations = operations
        self.calls = 0

    async def screenshot(self, destination: Path) -> SimpleNamespace:
        self.operations.append("screenshot")
        self.calls += 1
        destination.mkdir()
        (destination / "screen.png").write_bytes(b"png")
        return SimpleNamespace(directory=str(destination))


class FakeAsyncEvidence:
    def __init__(self, canonical: Path, operations: list[str]) -> None:
        self.canonical = canonical
        self.operations = operations
        self.complete_calls = 0

    async def complete(self, *, timeout: float) -> None:
        self.operations.append("complete")
        self.complete_calls += 1

    async def bundle(self, destination: Path, *, profile: str) -> SimpleNamespace:
        self.operations.append("bundle")
        destination.mkdir()
        (destination / "bundle.json").write_text("{}", encoding="utf-8")
        return SimpleNamespace(directory=str(destination))

    async def verify(self, level: str) -> SimpleNamespace:
        self.operations.append("verify")
        return SimpleNamespace(
            valid=True,
            session=str(self.canonical),
            artifact_roots_verified=1,
        )


class FakeAsyncSession:
    def __init__(self, root: Path, *, block_stop: bool = False) -> None:
        self.session_id = "async-session"
        self.canonical_session = root / "canonical"
        self.canonical_session.mkdir()
        (self.canonical_session / "events").write_bytes(b"events")
        self.operations: list[str] = []
        self.screen = FakeAsyncScreen(self.operations)
        self.evidence = FakeAsyncEvidence(self.canonical_session, self.operations)
        self.finish_calls = 0
        self.finish_passed: list[bool] = []
        self.stop_calls = 0
        self.stop_started = asyncio.Event()
        self.block_stop = block_stop

    async def stop(self) -> None:
        self.operations.append("stop")
        self.stop_calls += 1
        self.stop_started.set()
        if self.block_stop:
            await asyncio.Event().wait()

    async def finish(self, *, passed: bool) -> None:
        self.operations.append("finish")
        self.finish_calls += 1
        self.finish_passed.append(passed)


class FakeAsyncTerminal:
    def __init__(self, operations: list[str], *, close_error: bool = False) -> None:
        self.operations = operations
        self.close_error = close_error
        self.close_calls = 0
        self.closed = False

    async def close(self) -> None:
        self.operations.append("close")
        self.close_calls += 1
        if self.close_error:
            raise RuntimeError("async driver close broke")


@pytest.mark.asyncio
async def test_async_owner_awaits_exactly_one_finalization(tmp_path: Path) -> None:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        evidence_policy="always",
        run_id="0123456789abcdef0123456789abcdef",
        test_id="async-case",
        adapter="custom-async",
    )
    evidence = create_evidence_directory(config)
    session = FakeAsyncSession(evidence)
    terminal = FakeAsyncTerminal(session.operations)
    owner = AsyncTermtraceTest(
        config,
        cast(AsyncTerminal, terminal),
        cast(AsyncSession, session),
        evidence,
    )
    first = await owner.finalize(outcome="assertion_failed")
    second = await owner.finalize(outcome="passed")
    assert first is second
    assert session.evidence.complete_calls == 1
    assert session.finish_calls == 1
    assert terminal.close_calls == 1
    assert first.retained is True
    assert session.operations == [
        "stop",
        "complete",
        "screenshot",
        "bundle",
        "verify",
        "finish",
        "close",
    ]
    assert first.secondary_failures == ()
    assert len(first.verified_artifacts) == 3
    manifest = json.loads((evidence / "termtrace-test-evidence.json").read_text(encoding="utf-8"))
    assert manifest["adapter"] == "custom-async"


@pytest.mark.asyncio
async def test_async_finalization_propagates_cancellation_after_cleanup(
    tmp_path: Path,
) -> None:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        evidence_policy="always",
        run_id="0123456789abcdef0123456789abcdef",
        test_id="async-cancel",
    )
    evidence = create_evidence_directory(config)
    session = FakeAsyncSession(evidence, block_stop=True)
    terminal = FakeAsyncTerminal(session.operations)
    owner = AsyncTermtraceTest(
        config,
        cast(AsyncTerminal, terminal),
        cast(AsyncSession, session),
        evidence,
    )
    task = asyncio.create_task(owner.finalize(outcome="passed"))
    await session.stop_started.wait()

    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task
    assert terminal.close_calls == 1
    assert owner.report is not None
    assert await owner.finalize(outcome="passed") is owner.report


@pytest.mark.asyncio
async def test_async_driver_closes_before_passing_evidence_is_discarded(
    tmp_path: Path,
) -> None:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        evidence_policy="on_failure",
        run_id="0123456789abcdef0123456789abcdef",
        test_id="async-close-error",
    )
    evidence = create_evidence_directory(config)
    session = FakeAsyncSession(evidence)
    terminal = FakeAsyncTerminal(session.operations, close_error=True)
    owner = AsyncTermtraceTest(
        config,
        cast(AsyncTerminal, terminal),
        cast(AsyncSession, session),
        evidence,
    )

    report = await owner.finalize(outcome="passed")

    assert session.finish_passed == [False]
    assert session.operations == ["stop", "complete", "finish", "close"]
    assert report.retained is True
    assert evidence.exists()
    assert any("async driver close broke" in failure for failure in report.secondary_failures)


@pytest.mark.asyncio
async def test_async_manifest_uses_aggregate_hash_budget(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        evidence_policy="always",
        run_id="0123456789abcdef0123456789abcdef",
        test_id="async-hash-budget",
    )
    evidence = create_evidence_directory(config)
    session = FakeAsyncSession(evidence)
    terminal = FakeAsyncTerminal(session.operations)
    owner = AsyncTermtraceTest(
        config,
        cast(AsyncTerminal, terminal),
        cast(AsyncSession, session),
        evidence,
    )
    monkeypatch.setattr(
        testing_lifecycle,
        "_ArtifactHashBudget",
        lambda: _ArtifactHashBudget(bytes_remaining=1, members_remaining=100),
    )

    report = await owner.finalize(outcome="assertion_failed")

    assert any("artifact manifest exceeds" in failure for failure in report.secondary_failures)
    assert not (evidence / "termtrace-test-evidence.json").exists()
