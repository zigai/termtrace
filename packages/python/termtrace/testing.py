"""Runner-independent ownership, evidence, and snapshot policy for terminal tests."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import threading
import uuid
from collections.abc import Iterator, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from types import TracebackType
from typing import Literal, Self

from termtrace.assertions import (
    assert_cursor,
    assert_frame_text,
    assert_match_text,
    assert_matches,
    assert_no_matches,
    assert_snapshot_file,
)
from termtrace.client import Keyboard, Screen, Session, Terminal
from termtrace.environment import EnvironmentOptions
from termtrace.frames import ScreenFrame
from termtrace.locators import Locator, LocatorSpec, ScreenMatch
from termtrace.snapshots import FrameSnapshot, SnapshotProfile

EvidencePolicy = Literal["on_failure", "always", "manual"]
TestOutcome = Literal[
    "passed",
    "assertion_failed",
    "error",
    "panic_or_exception",
    "timed_out",
]
LifecycleState = Literal["open", "finalizing", "finalized"]
_MANIFEST_HASH_BYTE_BUDGET = 64 * 1024 * 1024
_MANIFEST_HASH_MEMBER_BUDGET = 10_000


@dataclass(slots=True)
class _ArtifactHashBudget:
    bytes_remaining: int = _MANIFEST_HASH_BYTE_BUDGET
    members_remaining: int = _MANIFEST_HASH_MEMBER_BUDGET

    def consume_member(self) -> None:
        if self.members_remaining <= 0:
            raise ValueError(f"artifact manifest exceeds {_MANIFEST_HASH_MEMBER_BUDGET} members")
        self.members_remaining -= 1

    def consume_bytes(self, size: int) -> None:
        if size > self.bytes_remaining:
            raise ValueError(f"artifact manifest exceeds {_MANIFEST_HASH_BYTE_BUDGET} hashed bytes")
        self.bytes_remaining -= size


@dataclass(frozen=True, slots=True)
class TermtraceTestConfig:
    command: tuple[str | Path, ...]
    evidence_root: Path
    snapshot_root: Path
    evidence_policy: EvidencePolicy = "on_failure"
    update_snapshots: bool = False
    binary_path: str | Path | None = None
    cols: int = 160
    rows: int = 45
    working_directory: str | Path | None = None
    environment: EnvironmentOptions | None = None
    output_byte_budget: int | None = None
    finalization_timeout: float = 10.0
    adapter: str = "pytest"
    run_id: str = ""
    worker_id: str = "main"
    test_id: str = "terminal-test"
    attempt: int = 0

    def with_command(self, command: Sequence[str | Path]) -> TermtraceTestConfig:
        return replace(self, command=tuple(command))


@dataclass(frozen=True, slots=True)
class FinalizationReport:
    outcome: TestOutcome
    evidence_directory: Path | None
    retained: bool
    artifacts: tuple[Path, ...]
    verified_artifacts: tuple[Path, ...]
    secondary_failures: tuple[str, ...]
    lifecycle_phases: tuple[str, ...]


class TerminalAssertions:
    """Runner-neutral assertions bound to one owned session."""

    def __init__(self, owner: TermtraceTest) -> None:
        self._owner = owner

    def screen_text(self, expected: str) -> None:
        assert_frame_text(self._owner.session.screen.frame(), expected)

    def matches(
        self,
        spec: LocatorSpec,
        *,
        count: int | None = None,
        frame: ScreenFrame | None = None,
    ) -> tuple[ScreenMatch, ...]:
        return assert_matches(
            frame or self._owner.session.screen.frame(),
            spec,
            count=count,
        )

    def no_matches(
        self,
        spec: LocatorSpec,
        *,
        frame: ScreenFrame | None = None,
    ) -> None:
        assert_no_matches(frame or self._owner.session.screen.frame(), spec)

    def match_text(self, match: ScreenMatch, expected: str) -> None:
        assert_match_text(match, expected)

    def cursor(
        self,
        *,
        row: int | None = None,
        column: int | None = None,
        visible: bool | None = None,
        frame: ScreenFrame | None = None,
    ) -> None:
        assert_cursor(
            frame or self._owner.session.screen.frame(),
            row=row,
            column=column,
            visible=visible,
        )

    def snapshot(
        self,
        name: str,
        *,
        profile: SnapshotProfile = "spans",
        frame: ScreenFrame | None = None,
        stable_for: float = 0.25,
    ) -> FrameSnapshot:
        return self._owner.assert_snapshot(
            name,
            profile=profile,
            frame=frame,
            stable_for=stable_for,
        )


class TermtraceTest:
    """Own exactly one synchronous driver and session for a test attempt."""

    def __init__(
        self,
        config: TermtraceTestConfig,
        terminal: Terminal,
        session: Session,
        evidence_directory: Path,
    ) -> None:
        self.config = config
        self._terminal = terminal
        self.session = session
        self.evidence_directory = evidence_directory
        self.assertions = TerminalAssertions(self)
        self._state: LifecycleState = "open"
        self._report: FinalizationReport | None = None
        self._lock = threading.Lock()
        self._snapshot_modes: dict[str, str] = {}

    @classmethod
    def start(cls, config: TermtraceTestConfig) -> TermtraceTest:
        if not config.command:
            raise ValueError("terminal test command must not be empty")
        evidence_directory = create_evidence_directory(config)
        terminal: Terminal | None = None
        try:
            terminal = Terminal(
                binary_path=config.binary_path,
                evidence_root=evidence_directory,
            )
            session = terminal.launch(
                config.command,
                cols=config.cols,
                rows=config.rows,
                working_directory=config.working_directory,
                evidence=_session_evidence_policy(config.evidence_policy),
                environment=config.environment,
                output_byte_budget=config.output_byte_budget,
            )
        except BaseException:
            if terminal is not None:
                terminal.close()
            shutil.rmtree(evidence_directory, ignore_errors=True)
            raise
        return cls(config, terminal, session, evidence_directory)

    @property
    def screen(self) -> Screen:
        return self.session.screen

    @property
    def keyboard(self) -> Keyboard:
        return self.session.keyboard

    def locator(self, spec: LocatorSpec) -> Locator:
        return self.session.screen.locator(spec)

    @property
    def report(self) -> FinalizationReport | None:
        return self._report

    def assert_snapshot(
        self,
        name: str,
        *,
        profile: SnapshotProfile = "spans",
        stable_for: float = 0.25,
        frame: ScreenFrame | None = None,
    ) -> FrameSnapshot:
        logical_name = _validate_snapshot_name(name)
        previous_mode = self._snapshot_modes.setdefault(logical_name, profile)
        if previous_mode != profile:
            raise ValueError(
                f"snapshot {logical_name!r} was already used with profile {previous_mode!r}"
            )
        if profile == "cells" and frame is None:
            raise ValueError("cells snapshots require an explicitly region-bounded ScreenFrame")
        selected = frame
        if selected is None:
            selected = self.session.screen.capture_stable(stable_for=stable_for).frame
        destination = self.config.snapshot_root / f"{logical_name}.json"
        return assert_snapshot_file(
            selected,
            destination,
            profile=profile,
            update=self.config.update_snapshots,
        )

    def finalize(
        self,
        *,
        outcome: TestOutcome,
        primary_failure: BaseException | None = None,
    ) -> FinalizationReport:
        interruption: BaseException | None = None
        with self._lock:
            if self._report is None:
                if self._state != "open":
                    raise RuntimeError("terminal test finalization state is invalid")
                self._state = "finalizing"
                self._report, interruption = self._finalize_once(outcome)
                self._state = "finalized"
            report = self._report
        if primary_failure is not None:
            for message in report.secondary_failures:
                primary_failure.add_note(f"Termtrace finalization: {message}")
        if interruption is not None:
            raise interruption
        return report

    def _recover_closed_driver(self, phases: list[str], secondary: list[str]) -> bool:
        if not self._terminal.closed:
            return False
        canonical_session = self.session.canonical_session
        try:
            self._terminal.close()
        except Exception as e:
            secondary.append(_bounded_error("closed driver cleanup", e))
        replacement: Terminal | None = None
        try:
            replacement = Terminal(
                binary_path=self.config.binary_path,
                evidence_root=self.evidence_directory,
            )
            session = replacement.attach(canonical_session)
        except Exception as e:
            if replacement is not None:
                replacement.close()
            secondary.append(_bounded_error("driver recovery", e))
            return False
        self._terminal = replacement
        self.session = session
        phases.append("driver_recovered")
        return True

    def _finalize_once(
        self, outcome: TestOutcome
    ) -> tuple[FinalizationReport, BaseException | None]:
        secondary: list[str] = []
        phases: list[str] = []
        artifacts: list[Path] = []
        verified_artifacts: list[Path] = []
        body_passed = outcome == "passed"
        completion_succeeded = False
        stop_error: Exception | None = None
        interruption: BaseException | None = None
        recovered_driver = self._recover_closed_driver(phases, secondary)
        try:
            self.session.stop(request_timeout=self.config.finalization_timeout)
            phases.append("stop_requested")
        except Exception as e:
            stop_error = e
        except BaseException as e:
            interruption = e

        if self._terminal.closed:
            recovered_driver = self._recover_closed_driver(phases, secondary) or recovered_driver
        try:
            self.session.evidence.complete(timeout=self.config.finalization_timeout)
            phases.append("completed")
            completion_succeeded = True
        except Exception as e:
            if stop_error is not None:
                secondary.append(_bounded_error("session stop", stop_error))
            secondary.append(_bounded_error("session completion", e))
        except BaseException as e:
            if interruption is None:
                interruption = e

        if not body_passed and completion_succeeded:
            screenshot = self.evidence_directory / "semantic-screenshot"
            try:
                result = self.session.screen.screenshot(
                    screenshot,
                    request_timeout=self.config.finalization_timeout,
                )
                screenshot_artifact = _contained_path(
                    self.evidence_directory, Path(result.directory)
                )
                artifacts.append(screenshot_artifact)
                verified_artifacts.append(screenshot_artifact)
                phases.append("semantic_screenshot")
            except Exception as e:
                secondary.append(_bounded_error("semantic screenshot", e))
            except BaseException as e:
                if interruption is None:
                    interruption = e

            bundle = self.evidence_directory / "screen-bundle"
            try:
                bundle_result = self.session.evidence.bundle(
                    bundle,
                    profile="screen",
                    request_timeout=self.config.finalization_timeout,
                )
                bundle_artifact = _contained_path(
                    self.evidence_directory, Path(bundle_result.directory)
                )
                artifacts.append(bundle_artifact)
                verified_artifacts.append(bundle_artifact)
                phases.append("screen_bundle")
            except Exception as e:
                secondary.append(_bounded_error("screen bundle", e))
            except BaseException as e:
                if interruption is None:
                    interruption = e
            try:
                verification = self.session.evidence.verify(
                    "content",
                    request_timeout=self.config.finalization_timeout,
                )
                if not verification.valid:
                    secondary.append("canonical evidence verifier reported invalid evidence")
                else:
                    canonical_artifact = _contained_path(
                        self.evidence_directory,
                        Path(verification.session),
                    )
                    artifacts.append(canonical_artifact)
                    verified_artifacts.append(canonical_artifact)
                    phases.append("canonical_verified")
            except Exception as e:
                secondary.append(_bounded_error("canonical verification", e))
            except BaseException as e:
                if interruption is None:
                    interruption = e

        if completion_succeeded:
            try:
                if recovered_driver:
                    self.session.close()
                    phases.append("session_detached")
                else:
                    self.session.finish(passed=False)
                    phases.append("session_finished")
            except Exception as e:
                secondary.append(_bounded_error("session finish", e))
            except BaseException as e:
                if interruption is None:
                    interruption = e
        try:
            self._terminal.close()
            phases.append("driver_closed")
        except Exception as e:
            secondary.append(_bounded_error("driver close", e))
        except BaseException as e:
            if interruption is None:
                interruption = e

        retained = (
            self.config.evidence_policy != "on_failure"
            or not body_passed
            or bool(secondary)
            or interruption is not None
        )
        manifest_path: Path | None = None
        if retained:
            try:
                manifest_path = self._write_manifest(outcome, artifacts, secondary, phases)
                artifacts.append(manifest_path)
                phases.append("manifest_written")
            except Exception as e:
                secondary.append(_bounded_error("adapter manifest", e))
            except BaseException as e:
                if interruption is None:
                    interruption = e
        else:
            try:
                shutil.rmtree(self.evidence_directory, ignore_errors=False)
                phases.append("evidence_discarded")
            except Exception as e:
                retained = True
                secondary.append(_bounded_error("evidence discard", e))
                try:
                    manifest_path = self._write_manifest(outcome, artifacts, secondary, phases)
                    artifacts.append(manifest_path)
                    phases.append("manifest_written")
                except Exception as manifest_error:
                    secondary.append(_bounded_error("adapter manifest", manifest_error))
                except BaseException as e:
                    if interruption is None:
                        interruption = e
            except BaseException as e:
                retained = True
                if interruption is None:
                    interruption = e

        report = FinalizationReport(
            outcome=outcome,
            evidence_directory=self.evidence_directory if retained else None,
            retained=retained,
            artifacts=tuple(artifacts),
            verified_artifacts=tuple(verified_artifacts),
            secondary_failures=tuple(secondary),
            lifecycle_phases=tuple(phases),
        )
        return report, interruption

    def _write_manifest(
        self,
        outcome: TestOutcome,
        artifacts: list[Path],
        secondary: list[str],
        phases: list[str],
    ) -> Path:
        relative_artifacts: list[dict[str, str]] = []
        budget = _ArtifactHashBudget()
        for artifact in artifacts:
            relative = artifact.relative_to(self.evidence_directory)
            relative_artifacts.append(
                {"path": relative.as_posix(), "sha256": _artifact_digest(artifact, budget)}
            )
        manifest = {
            "format": "termtrace-test-evidence-v1",
            "adapter": self.config.adapter,
            "run_id": _evidence_run_id(self.config, self.evidence_directory),
            "worker_id": self.config.worker_id,
            "test_hash": _test_hash(self.config.test_id),
            "attempt": self.config.attempt,
            "session_id": self.session.session_id,
            "outcome": outcome,
            "retention_reason": "policy" if outcome == "passed" else "test_failure",
            "artifacts": relative_artifacts,
            "secondary_errors": secondary[:16],
            "lifecycle_phases": phases,
        }
        destination = self.evidence_directory / "termtrace-test-evidence.json"
        destination.write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return destination

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> Literal[False]:
        outcome = classify_outcome(error)
        report = self.finalize(outcome=outcome, primary_failure=error)
        if error is None and report.secondary_failures:
            raise RuntimeError("; ".join(report.secondary_failures))
        return False


def classify_outcome(error: BaseException | None) -> TestOutcome:
    if error is None:
        return "passed"
    if isinstance(error, AssertionError):
        return "assertion_failed"
    if isinstance(error, TimeoutError):
        return "timed_out"
    if isinstance(error, Exception):
        return "error"
    return "panic_or_exception"


def create_evidence_directory(config: TermtraceTestConfig) -> Path:
    root = config.evidence_root.absolute()
    _reject_symlink_ancestors(root)
    root.mkdir(parents=True, exist_ok=True)
    _reject_symlink_ancestors(root)
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,31}", config.adapter):
        raise ValueError("adapter must be a fixed safe identifier")
    if config.attempt < 0:
        raise ValueError("attempt must not be negative")
    run_id = config.run_id or uuid.uuid4().hex
    if not re.fullmatch(r"[a-zA-Z0-9_-]{16,128}", run_id):
        raise ValueError("run_id must be 16-128 safe characters")
    worker = f"{_slug(config.worker_id, 32)}-{_test_hash(config.worker_id)[:12]}"
    test_component = f"{_slug(config.test_id, 48)}-{_test_hash(config.test_id)}"
    session_id = uuid.uuid4().hex
    directory = (
        root
        / config.adapter
        / run_id
        / worker
        / test_component
        / f"attempt-{config.attempt}"
        / session_id
    )
    directory.mkdir(parents=True, exist_ok=False)
    _reject_symlink_ancestors(directory)
    return _contained_path(root, directory)


def _reject_symlink_ancestors(path: Path) -> None:
    for candidate in (path, *path.parents):
        if not candidate.exists():
            continue
        if candidate.is_symlink():
            raise ValueError(f"evidence path contains a symlink: {candidate}")
        if not candidate.is_dir():
            raise ValueError(f"evidence path ancestor is not a directory: {candidate}")


def _session_evidence_policy(
    policy: EvidencePolicy,
) -> Literal["always", "on-failure"]:
    if policy == "on_failure":
        return "on-failure"
    return "always"


def _validate_snapshot_name(name: str) -> str:
    if not name or name in {".", ".."}:
        raise ValueError("snapshot name must be a non-empty logical name")
    if Path(name).is_absolute() or "/" in name or "\\" in name:
        raise ValueError("snapshot name must not contain a path")
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}", name):
        raise ValueError("snapshot name contains unsupported characters")
    return name


def _contained_path(root: Path, path: Path) -> Path:
    resolved_root = root.resolve(strict=True)
    resolved_path = path.resolve(strict=True)
    if not resolved_path.is_relative_to(resolved_root):
        raise ValueError(f"artifact path escapes evidence root: {path}")
    return resolved_path


def _slug(value: str, limit: int) -> str:
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", value).strip("-").lower()
    return (slug or "test")[:limit]


def _test_hash(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()[:24]


def _evidence_run_id(config: TermtraceTestConfig, directory: Path) -> str:
    relative = directory.resolve(strict=True).relative_to(config.evidence_root.resolve(strict=True))
    if len(relative.parts) < 2 or relative.parts[0] != config.adapter:
        raise ValueError("evidence directory does not match adapter layout")
    return relative.parts[1]


def _artifact_digest(path: Path, budget: _ArtifactHashBudget | None = None) -> str:
    digest = hashlib.sha256()
    active_budget = budget or _ArtifactHashBudget()
    if path.is_symlink() or not (path.is_file() or path.is_dir()):
        raise ValueError(f"artifact contains an unsupported file: {path}")
    if path.is_file():
        members: Iterator[Path] = iter((path,))
        relative_root = path.parent
    else:
        relative_root = path

        def walk_members() -> Iterator[Path]:
            for root, directories, filenames in os.walk(path):
                directories.sort()
                filenames.sort()
                root_path = Path(root)
                for name in directories:
                    yield root_path / name
                for name in filenames:
                    yield root_path / name

        members = walk_members()
    for member in members:
        active_budget.consume_member()
        if member.is_symlink() or (not member.is_file() and not member.is_dir()):
            raise ValueError(f"artifact contains an unsupported file: {member}")
        if not member.is_file():
            continue
        digest.update(member.relative_to(relative_root).as_posix().encode())
        with member.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                active_budget.consume_bytes(len(chunk))
                digest.update(chunk)
    return digest.hexdigest()


def _bounded_error(phase: str, e: BaseException) -> str:
    detail = str(e).replace("\n", " ")[:512]
    return f"{phase} failed: {type(e).__name__}: {detail}"
