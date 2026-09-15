from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


def run_nested_pytest(
    project: Path,
    *arguments: str,
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    child_environment: dict[str, str] = os.environ.copy()
    child_environment["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    if environment is not None:
        child_environment.update(environment)
    return subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-p",
            "termtrace.pytest_plugin",
            *arguments,
        ],
        cwd=project,
        env=child_environment,
        check=False,
        capture_output=True,
        text=True,
    )


def test_statically_skipped_test_does_not_launch(tmp_path: Path) -> None:
    (tmp_path / "test_skipped.py").write_text(
        """
import pytest

@pytest.mark.skip(reason="not selected")
@pytest.mark.termtrace(command=["binary-that-must-not-be-resolved"])
def test_skipped(termtrace_test):
    raise AssertionError("body must not run")
""",
        encoding="utf-8",
    )
    result = run_nested_pytest(tmp_path, "-q")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 skipped" in result.stdout
    assert not (tmp_path / ".termtrace-evidence").exists()


def test_dynamically_skipped_test_treated_as_passed(tmp_path: Path) -> None:
    (tmp_path / "test_dyn_skipped.py").write_text(
        """
import pytest

def test_dynamic_skip(termtrace_test_factory):
    pytest.skip("skipping dynamically")
""",
        encoding="utf-8",
    )
    result = run_nested_pytest(tmp_path, "-q")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 skipped" in result.stdout


def test_filtered_test_does_not_launch(tmp_path: Path) -> None:
    (tmp_path / "test_filtered.py").write_text(
        """
import pytest

@pytest.mark.termtrace(command=["binary-that-must-not-be-resolved"])
def test_filtered(termtrace_test):
    raise AssertionError("body must not run")

def test_selected():
    pass
""",
        encoding="utf-8",
    )
    result = run_nested_pytest(tmp_path, "-q", "-k", "selected")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 passed" in result.stdout
    assert not (tmp_path / ".termtrace-evidence").exists()


def test_teardown_failure_retains_deferred_on_failure_evidence(tmp_path: Path) -> None:
    evidence_root = tmp_path / "evidence"
    (tmp_path / "test_teardown_retention.py").write_text(
        """
from pathlib import Path

import pytest

from termtrace import pytest_plugin
from termtrace.testing import FinalizationReport


class FakeOwner:
    def __init__(self, config):
        self.config = config
        self.evidence_directory = config.evidence_root / config.test_id.split("::")[-1]
        self.evidence_directory.mkdir(parents=True)

    def finalize(self, *, outcome):
        manifest = self.evidence_directory / "termtrace-test-evidence.json"
        manifest.write_text('{"retained": true}\\n', encoding="utf-8")
        return FinalizationReport(
            outcome=outcome,
            evidence_directory=self.evidence_directory,
            retained=True,
            artifacts=(manifest,),
            verified_artifacts=(),
            secondary_failures=(),
            lifecycle_phases=("manifest_written",),
        )


@pytest.fixture(autouse=True)
def fake_owner(monkeypatch):
    monkeypatch.setattr(pytest_plugin.TermtraceTest, "start", FakeOwner)


@pytest.fixture
def broken_teardown():
    yield
    raise RuntimeError("fixture teardown broke")


def test_teardown_failure(termtrace_test_factory, broken_teardown):
    termtrace_test_factory(["ignored"])


def test_passing_attempt(termtrace_test_factory):
    termtrace_test_factory(["ignored"])
""",
        encoding="utf-8",
    )

    result = run_nested_pytest(
        tmp_path,
        "-q",
        "--termtrace-evidence-root",
        str(evidence_root),
    )

    assert result.returncode != 0
    assert "fixture teardown broke" in result.stdout
    manifests: list[Path] = list(evidence_root.rglob("termtrace-test-evidence.json"))
    assert len(manifests) == 1
    assert "test_teardown_failure" in manifests[0].parts
    assert "Termtrace evidence:" in result.stdout


def test_real_pytest_sync_async_snapshot_failure_and_timeout_lifecycle(
    tmp_path: Path,
) -> None:
    repository_root = Path(__file__).resolve().parents[3]
    build = subprocess.run(
        ["cargo", "build", "--locked", "--bin", "termtrace"],
        cwd=repository_root,
        check=False,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stdout + build.stderr
    binary = repository_root / "target" / "debug" / "termtrace"
    evidence_root = tmp_path / "evidence"
    environment: dict[str, str] = {
        "TERMTRACE_BINARY": str(binary),
    }
    (tmp_path / "test_lifecycle.py").write_text(
        """
import os

import pytest

from termtrace import Duration

BINARY = os.environ["TERMTRACE_BINARY"]
COMMAND = ["/bin/sh", "-c", "trap 'exit 0' TERM; printf READY; while :; do sleep .1; done"]

@pytest.mark.termtrace(command=COMMAND, binary_path=BINARY)
def test_sync_ready(termtrace_test):
    termtrace_test.session.screen.wait_for_text("READY", timeout=5)
    termtrace_test.assertions.screen_text("READY")
    termtrace_test.assertions.snapshot("sync-ready", stable_for=0.02)

@pytest.mark.asyncio
@pytest.mark.termtrace(command=COMMAND, binary_path=BINARY)
async def test_async_ready(async_termtrace_test):
    await async_termtrace_test.session.screen.wait_for_text("READY", timeout=5)
    await async_termtrace_test.assertions.screen_text("READY")
    await async_termtrace_test.assertions.snapshot("async-ready", stable_for=0.02)

@pytest.mark.termtrace(command=COMMAND, binary_path=BINARY)
def test_never_matches(termtrace_test):
    termtrace_test.screen.get_by_text("NEVER").wait(
        timeout=Duration.milliseconds(30),
        interval=Duration.milliseconds(1),
    )
""",
        encoding="utf-8",
    )

    passing = run_nested_pytest(
        tmp_path,
        "-q",
        "-p",
        "pytest_asyncio.plugin",
        "-k",
        "sync_ready or async_ready",
        "--termtrace-update-snapshots",
        "--termtrace-evidence-root",
        str(evidence_root),
        environment=environment,
    )
    assert passing.returncode == 0, passing.stdout + passing.stderr
    assert "2 passed" in passing.stdout
    assert not any(path.is_file() for path in evidence_root.rglob("*"))

    sync_snapshot = next(tmp_path.rglob("sync-ready.json"))
    original = sync_snapshot.read_text(encoding="utf-8")
    sync_snapshot.write_text(original.replace("READY", "STALE"), encoding="utf-8")
    mismatch = run_nested_pytest(
        tmp_path,
        "-q",
        "-p",
        "pytest_asyncio.plugin",
        "-k",
        "test_sync_ready",
        "--termtrace-evidence-root",
        str(evidence_root),
        environment=environment,
    )
    assert mismatch.returncode != 0
    assert "terminal snapshot differs" in mismatch.stdout
    assert "STALE" in mismatch.stdout
    assert "READY" in mismatch.stdout
    assert "Termtrace evidence:" in mismatch.stdout

    async_snapshot = next(tmp_path.rglob("async-ready.json"))
    original = async_snapshot.read_text(encoding="utf-8")
    async_snapshot.write_text(original.replace("READY", "STALE"), encoding="utf-8")
    async_mismatch = run_nested_pytest(
        tmp_path,
        "-q",
        "-p",
        "pytest_asyncio.plugin",
        "-k",
        "test_async_ready",
        "--termtrace-evidence-root",
        str(evidence_root),
        environment=environment,
    )
    assert async_mismatch.returncode != 0
    assert "terminal snapshot differs" in async_mismatch.stdout
    assert "Termtrace evidence:" in async_mismatch.stdout

    timeout = run_nested_pytest(
        tmp_path,
        "-q",
        "-p",
        "pytest_asyncio.plugin",
        "-k",
        "never_matches",
        "--termtrace-evidence-root",
        str(evidence_root),
        environment=environment,
    )
    assert timeout.returncode != 0
    assert "locator wait reached its deadline" in timeout.stdout
    assert "Termtrace evidence:" in timeout.stdout

    manifests: list[Path] = list(evidence_root.rglob("termtrace-test-evidence.json"))
    assert len(manifests) == 3
    adapters: list[str] = []
    for manifest_path in manifests:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        adapters.append(manifest["adapter"])
        assert manifest["retention_reason"] == "test_failure"
        assert "semantic_screenshot" in manifest["lifecycle_phases"]
        assert "screen_bundle" in manifest["lifecycle_phases"]
        assert "canonical_verified" in manifest["lifecycle_phases"]
        attempt_root = manifest_path.parent
        assert next(attempt_root.rglob("screen.png")).is_file()
        assert next(attempt_root.rglob("screen-bundle/manifest.json")).is_file()
        completion_files: list[Path] = list(attempt_root.rglob("completion.json"))
        assert completion_files
        assert not list(attempt_root.rglob("control.sock"))
    assert adapters.count("pytest") == 2
    assert adapters.count("pytest-asyncio") == 1
