from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

from termtrace.session.sync import Session
from termtrace.testing.lifecycle import (
    EvidencePolicy,
    TermtraceTestConfig,
    _artifact_digest,
    _ArtifactHashBudget,
    create_evidence_directory,
)
from termtrace.testing.sync import TermtraceTest
from termtrace.transport.sync import DriverTransport, Terminal


class FakeScreen:
    def __init__(self, root: Path, operations: list[str]) -> None:
        self.root = root
        self.operations = operations
        self.screenshot_calls = 0

    def screenshot(
        self, destination: Path, *, request_timeout: float | None = None
    ) -> SimpleNamespace:
        self.operations.append("screenshot")
        self.screenshot_calls += 1
        destination.mkdir()
        (destination / "screen.png").write_bytes(b"png")
        return SimpleNamespace(directory=str(destination))

    def capture_stable(self, *, stable_for: float) -> SimpleNamespace:
        return SimpleNamespace(frame=object())


class FakeEvidence:
    def __init__(
        self,
        root: Path,
        canonical: Path,
        operations: list[str],
        *,
        completion_error: bool = False,
    ) -> None:
        self.root = root
        self.canonical = canonical
        self.completion_error = completion_error
        self.operations = operations
        self.complete_calls = 0

    def complete(self, *, timeout: float) -> None:
        self.operations.append("complete")
        self.complete_calls += 1
        if self.completion_error:
            raise RuntimeError("cleanup broke")

    def bundle(
        self,
        destination: Path,
        *,
        profile: str,
        request_timeout: float | None = None,
    ) -> SimpleNamespace:
        self.operations.append("bundle")
        destination.mkdir()
        (destination / "bundle.json").write_text("{}", encoding="utf-8")
        return SimpleNamespace(directory=str(destination))

    def verify(self, level: str, *, request_timeout: float | None = None) -> SimpleNamespace:
        self.operations.append("verify")
        return SimpleNamespace(
            valid=True,
            session=str(self.canonical),
            artifact_roots_verified=1,
        )


class FakeSession:
    def __init__(
        self,
        root: Path,
        *,
        completion_error: bool = False,
        stop_interruption: BaseException | None = None,
    ) -> None:
        self.session_id = "session-id"
        self.canonical_session = root / "canonical"
        self.canonical_session.mkdir()
        (self.canonical_session / "events").write_bytes(b"events")
        self.operations: list[str] = []
        self.screen = FakeScreen(root, self.operations)
        self.evidence = FakeEvidence(
            root,
            self.canonical_session,
            self.operations,
            completion_error=completion_error,
        )
        self.finish_calls = 0
        self.finish_passed: list[bool] = []
        self.stop_calls = 0
        self.stop_interruption = stop_interruption

    def stop(self, *, request_timeout: float | None = None) -> None:
        self.operations.append("stop")
        self.stop_calls += 1
        if self.stop_interruption is not None:
            raise self.stop_interruption

    def finish(self, *, passed: bool) -> None:
        self.operations.append("finish")
        self.finish_calls += 1
        self.finish_passed.append(passed)


class FakeTerminal:
    def __init__(self, operations: list[str], *, close_error: bool = False) -> None:
        self.operations = operations
        self.close_error = close_error
        self.close_calls = 0
        self.closed = False

    def close(self) -> None:
        self.operations.append("close")
        self.close_calls += 1
        if self.close_error:
            raise RuntimeError("driver close broke")


def make_owner(
    tmp_path: Path,
    *,
    policy: EvidencePolicy = "always",
    completion_error: bool = False,
    close_error: bool = False,
    stop_interruption: BaseException | None = None,
    update_snapshots: bool = False,
    adapter: str = "pytest",
) -> tuple[TermtraceTest, FakeTerminal, FakeSession]:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        evidence_policy=policy,
        update_snapshots=update_snapshots,
        run_id="0123456789abcdef0123456789abcdef",
        adapter=adapter,
        test_id="tests/test_adapter.py::test_case[value]",
    )
    evidence = create_evidence_directory(config)
    session = FakeSession(
        evidence,
        completion_error=completion_error,
        stop_interruption=stop_interruption,
    )
    terminal = FakeTerminal(session.operations, close_error=close_error)
    owner = TermtraceTest(
        config,
        cast(Terminal, terminal),
        cast(Session, session),
        evidence,
    )
    return owner, terminal, session


def test_root_import_does_not_load_pytest(tmp_path: Path) -> None:
    script = tmp_path / "import_root.py"
    script.write_text(
        """
import importlib.abc
import sys

class RejectPytest(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path, target=None):
        if fullname == "pytest" or fullname.startswith("pytest."):
            raise RuntimeError("pytest import leaked")
        return None

sys.meta_path.insert(0, RejectPytest())
import termtrace
assert "pytest" not in sys.modules
""",
        encoding="utf-8",
    )
    result = subprocess.run(
        [sys.executable, str(script)],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def test_evidence_paths_are_collision_free_and_contained(tmp_path: Path) -> None:
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path,
        snapshot_root=tmp_path / "snapshots",
        run_id="0123456789abcdef0123456789abcdef",
        worker_id="gw0/../../other",
        test_id="tests/../same title[param/../../x]",
    )
    first = create_evidence_directory(config)
    second = create_evidence_directory(config)
    assert first != second
    assert first.is_relative_to(tmp_path)
    assert second.is_relative_to(tmp_path)
    assert ".." not in first.parts


def test_evidence_root_rejects_symlink_traversal(tmp_path: Path) -> None:
    real_root = tmp_path / "real"
    real_root.mkdir()
    linked_root = tmp_path / "linked"
    linked_root.symlink_to(real_root, target_is_directory=True)
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=linked_root,
        snapshot_root=tmp_path / "snapshots",
        run_id="0123456789abcdef0123456789abcdef",
    )
    with pytest.raises(ValueError, match="symlink"):
        create_evidence_directory(config)


def test_finalize_is_exactly_once_and_on_failure_discards_pass(tmp_path: Path) -> None:
    owner, terminal, session = make_owner(tmp_path, policy="on_failure")
    first = owner.finalize(outcome="passed")
    second = owner.finalize(outcome="assertion_failed")
    assert first is second
    assert session.evidence.complete_calls == 1
    assert session.finish_calls == 1
    assert terminal.close_calls == 1
    assert not owner.evidence_directory.exists()
    assert first.retained is False


def test_primary_error_is_preserved_with_cleanup_as_note(tmp_path: Path) -> None:
    owner, terminal, session = make_owner(tmp_path, completion_error=True)
    primary = ValueError("body failed")
    with pytest.raises(ValueError) as captured, owner:
        raise primary
    assert captured.value is primary
    assert any("cleanup broke" in note for note in primary.__notes__)
    assert owner.report is not None
    assert owner.report.retained is True
    assert session.finish_calls == 0
    assert terminal.close_calls == 1


@pytest.mark.parametrize("interruption", [KeyboardInterrupt(), SystemExit(7)])
def test_finalization_propagates_interruption_after_cleanup(
    tmp_path: Path, interruption: BaseException
) -> None:
    owner, terminal, _ = make_owner(tmp_path, stop_interruption=interruption)

    with pytest.raises(type(interruption)) as captured:
        owner.finalize(outcome="passed")

    assert captured.value is interruption
    assert terminal.close_calls == 1
    assert owner.report is not None
    assert owner.finalize(outcome="passed") is owner.report


def test_finalization_aborts_a_silent_stop_transport_at_its_deadline(
    tmp_path: Path,
) -> None:
    driver = tmp_path / "silent_driver.py"
    driver.write_text(
        f"""#!{sys.executable}
import sys

sys.stdin.buffer.readline()
sys.stdin.buffer.readline()
""",
        encoding="utf-8",
    )
    driver.chmod(0o700)
    config = TermtraceTestConfig(
        command=("ignored",),
        evidence_root=tmp_path / "evidence",
        snapshot_root=tmp_path / "snapshots",
        evidence_policy="always",
        finalization_timeout=0.02,
        run_id="0123456789abcdef0123456789abcdef",
        test_id="tests/test_adapter.py::test_silent_stop",
    )
    evidence = create_evidence_directory(config)
    canonical = evidence / "canonical"
    canonical.mkdir()
    transport = DriverTransport(driver)
    session = Session(
        transport,
        "handle",
        "session-id",
        str(canonical),
        owned=True,
        evidence_policy="always",
        completed=False,
    )
    terminal = FakeTerminal([])
    owner = TermtraceTest(
        config,
        cast(Terminal, terminal),
        session,
        evidence,
    )

    started_at = time.monotonic()
    report = owner.finalize(outcome="passed")
    elapsed = time.monotonic() - started_at

    assert elapsed < 0.5
    assert terminal.close_calls == 1
    assert "driver_closed" in report.lifecycle_phases
    assert any("session stop" in failure for failure in report.secondary_failures)
    assert any("deadline" in failure for failure in report.secondary_failures)


def test_driver_closes_before_passing_evidence_is_discarded(tmp_path: Path) -> None:
    owner, terminal, session = make_owner(
        tmp_path,
        policy="on_failure",
        close_error=True,
    )

    report = owner.finalize(outcome="passed")

    assert session.finish_passed == [False]
    assert session.operations == ["stop", "complete", "finish", "close"]
    assert terminal.close_calls == 1
    assert report.retained is True
    assert owner.evidence_directory.exists()
    assert any("driver close broke" in failure for failure in report.secondary_failures)


def test_artifact_digest_enforces_aggregate_hash_budget(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.write_bytes(b"abc")
    second.write_bytes(b"d")
    budget = _ArtifactHashBudget(bytes_remaining=3, members_remaining=2)

    _artifact_digest(first, budget)

    with pytest.raises(ValueError, match="hashed bytes"):
        _artifact_digest(second, budget)


def test_failure_artifacts_follow_completion_and_are_verified(tmp_path: Path) -> None:
    owner, _, session = make_owner(tmp_path, adapter="custom-sync")
    report = owner.finalize(outcome="assertion_failed")
    assert report.evidence_directory is not None
    manifest = json.loads(
        (report.evidence_directory / "termtrace-test-evidence.json").read_text(encoding="utf-8")
    )
    assert session.operations == [
        "stop",
        "complete",
        "screenshot",
        "bundle",
        "verify",
        "finish",
        "close",
    ]
    assert report.secondary_failures == ()
    assert len(report.verified_artifacts) == 3
    assert manifest["format"] == "termtrace-test-evidence-v1"
    assert manifest["adapter"] == "custom-sync"
    assert all(not Path(item["path"]).is_absolute() for item in manifest["artifacts"])
