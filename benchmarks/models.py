#!/usr/bin/env python3
"""Structured data models and emitters for Termtrace benchmarks."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass, field
import json
import hashlib
from pathlib import Path
import subprocess
import sys
from typing import Any


@dataclass(frozen=True, slots=True)
class BenchmarkRunManifest:
    command: str
    source_checkout_commit: str
    binary: str
    binary_sha256: str
    build_revision: str
    build_profile: str
    host: str
    started_unix_ns: int
    format: str = "termtrace-benchmark-run"
    version: int = 1


@dataclass(frozen=True, slots=True)
class AgentWorkflowResult:
    workflow: str
    stdout_bytes: int
    elapsed_ns: int
    maximum_stdout_bytes: int
    command_count: int
    maximum_commands: int
    passed: bool


@dataclass(frozen=True, slots=True)
class DurabilityResult:
    mode: str
    iteration: int
    payload_bytes: int
    elapsed_ns: int
    maximum_rss_bytes: int
    durability: Any


@dataclass(frozen=True, slots=True)
class RenderEditClipTime:
    monotonicNs: int


@dataclass(frozen=True, slots=True)
class RenderEditClip:
    from_: RenderEditClipTime
    to: RenderEditClipTime
    caption: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "from": {"monotonicNs": self.from_.monotonicNs},
            "to": {"monotonicNs": self.to.monotonicNs},
            "caption": self.caption,
        }


@dataclass(frozen=True, slots=True)
class RenderEditManifest:
    version: int = 1
    clips: list[RenderEditClip] = field(default_factory=list)
    change_visualization: None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": self.version,
            "clips": [clip.to_dict() for clip in self.clips],
            "change_visualization": self.change_visualization,
        }


@dataclass(frozen=True, slots=True)
class RenderResult:
    operation: str
    iteration: int
    elapsed_ns: int
    artifact_hash: str


def main() -> None:
    parser = argparse.ArgumentParser(description="Termtrace benchmark model serializer")
    subparsers = parser.add_subparsers(dest="subcommand", required=True)

    # emit-run-manifest
    manifest_p = subparsers.add_parser("emit-run-manifest")
    manifest_p.add_argument("--command", required=True)
    manifest_p.add_argument("--revision", required=True)
    manifest_p.add_argument("--binary", required=True)
    manifest_p.add_argument("--started-unix-ns", type=int, required=True)
    manifest_p.add_argument("--host", required=True)

    # emit-agent-workflow-result
    workflow_p = subparsers.add_parser("emit-agent-workflow-result")
    workflow_p.add_argument("--workflow", required=True)
    workflow_p.add_argument("--stdout-bytes", type=int, required=True)
    workflow_p.add_argument("--elapsed-ns", type=int, required=True)
    workflow_p.add_argument("--max-stdout-bytes", type=int, required=True)
    workflow_p.add_argument("--command-count", type=int, required=True)
    workflow_p.add_argument("--max-commands", type=int, required=True)

    # emit-durability-result
    durability_p = subparsers.add_parser("emit-durability-result")
    durability_p.add_argument("--mode", required=True)
    durability_p.add_argument("--iteration", type=int, required=True)
    durability_p.add_argument("--payload-bytes", type=int, required=True)
    durability_p.add_argument("--elapsed-ns", type=int, required=True)
    durability_p.add_argument("--max-rss-kib", type=int, required=True)
    durability_p.add_argument("--completion-file", required=True)

    # emit-render-edit
    edit_p = subparsers.add_parser("emit-render-edit")
    edit_p.add_argument("--end-ns", type=int, required=True)

    # emit-render-result
    render_p = subparsers.add_parser("emit-render-result")
    render_p.add_argument("--operation", required=True)
    render_p.add_argument("--iteration", type=int, required=True)
    render_p.add_argument("--elapsed-ns", type=int, required=True)
    render_p.add_argument("--artifact-hash", required=True)

    args = parser.parse_args()

    if args.subcommand == "emit-run-manifest":
        binary = Path(args.binary).resolve()
        version = json.loads(subprocess.check_output(
            [str(binary), "version", "--json"], timeout=30
        ))
        if isinstance(version, list) and len(version) == 1:
            version = version[0]
        if not isinstance(version, dict) or not isinstance(version.get("build_revision"), str):
            raise ValueError("binary did not report its build revision")
        with binary.open("rb") as stream:
            binary_hash = hashlib.file_digest(stream, "sha256").hexdigest()
        manifest = BenchmarkRunManifest(
            command=args.command,
            source_checkout_commit=args.revision,
            binary=str(binary),
            binary_sha256=binary_hash,
            build_revision=version["build_revision"],
            build_profile="unverified-existing-artifact",
            host=args.host,
            started_unix_ns=args.started_unix_ns,
        )
        print(json.dumps(asdict(manifest), indent=2))
    elif args.subcommand == "emit-agent-workflow-result":
        passed = (
            args.stdout_bytes <= args.max_stdout_bytes
            and args.command_count <= args.max_commands
        )
        result = AgentWorkflowResult(
            workflow=args.workflow,
            stdout_bytes=args.stdout_bytes,
            elapsed_ns=args.elapsed_ns,
            maximum_stdout_bytes=args.max_stdout_bytes,
            command_count=args.command_count,
            maximum_commands=args.max_commands,
            passed=passed,
        )
        print(json.dumps(asdict(result), separators=(",", ":")))
    elif args.subcommand == "emit-durability-result":
        completion_data = json.loads(
            Path(args.completion_file).read_text(encoding="utf-8")
        )
        durability_info = (
            completion_data[0]["durability"]
            if isinstance(completion_data, list)
            else completion_data["durability"]
        )
        durability = DurabilityResult(
            mode=args.mode,
            iteration=args.iteration,
            payload_bytes=args.payload_bytes,
            elapsed_ns=args.elapsed_ns,
            maximum_rss_bytes=args.max_rss_kib * 1024,
            durability=durability_info,
        )
        print(json.dumps(asdict(durability), separators=(",", ":")))
    elif args.subcommand == "emit-render-edit":
        edit = RenderEditManifest(
            clips=[
                RenderEditClip(
                    from_=RenderEditClipTime(monotonicNs=0),
                    to=RenderEditClipTime(monotonicNs=args.end_ns),
                    caption=None,
                )
            ],
            change_visualization=None,
        )
        print(json.dumps(edit.to_dict(), indent=2))
    elif args.subcommand == "emit-render-result":
        render = RenderResult(
            operation=args.operation,
            iteration=args.iteration,
            elapsed_ns=args.elapsed_ns,
            artifact_hash=args.artifact_hash,
        )
        print(json.dumps(asdict(render), separators=(",", ":")))


if __name__ == "__main__":
    main()
