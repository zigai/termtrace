"""Pure test configuration, lifecycle decisions, validation, and evidence helpers."""

from __future__ import annotations

import hashlib
import os
import re
import uuid
from collections.abc import Iterator, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Literal

from termtrace.environment import EnvironmentOptions

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


__all__ = [
    "EvidencePolicy",
    "FinalizationReport",
    "LifecycleState",
    "TermtraceTestConfig",
    "TestOutcome",
    "classify_outcome",
    "create_evidence_directory",
]
