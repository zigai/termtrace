#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import NotRequired, TypedDict


class Dependency(TypedDict):
    name: str
    path: NotRequired[str]


class Package(TypedDict):
    id: str
    name: str
    manifest_path: str
    dependencies: list[Dependency]


class Metadata(TypedDict):
    packages: list[Package]
    workspace_members: list[str]


ROOT = Path(__file__).resolve().parent.parent
ALLOWED_DEPENDENCIES: dict[str, frozenset[str]] = {
    "termtrace": frozenset(
        {
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-scenario",
            "termtrace-schema",
            "termtrace-sdk",
            "termtrace-service",
            "termtrace-terminal",
            "termtrace-tmux",
        }
    ),
    "termtrace-control": frozenset({"termtrace-model", "termtrace-process"}),
    "termtrace-engine": frozenset(
        {
            "termtrace-control",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
            "termtrace-terminal",
        }
    ),
    "termtrace-evidence": frozenset({"termtrace-model"}),
    "termtrace-export": frozenset(
        {
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-render",
            "termtrace-tmux",
        }
    ),
    "termtrace-media": frozenset(
        {
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-terminal",
        }
    ),
    "termtrace-model": frozenset(),
    "termtrace-process": frozenset(),
    "termtrace-render": frozenset({"termtrace-model", "termtrace-terminal"}),
    "termtrace-node": frozenset({"termtrace-model", "termtrace-sdk"}),
    "termtrace-scenario": frozenset(
        {
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-render",
            "termtrace-service",
        }
    ),
    "termtrace-python": frozenset({"termtrace-model", "termtrace-sdk"}),
    "termtrace-schema": frozenset({"termtrace-model", "termtrace-tmux"}),
    "termtrace-sdk": frozenset(
        {
            "termtrace-control",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-scenario",
            "termtrace-service",
        }
    ),
    "termtrace-service": frozenset(
        {
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-terminal",
            "termtrace-tmux",
        }
    ),
    "termtrace-terminal": frozenset({"termtrace-model"}),
    "termtrace-tmux": frozenset(
        {
            "termtrace-control",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
        }
    ),
    "termtrace-test-fixture": frozenset(
        {"termtrace-engine", "termtrace-evidence", "termtrace-model"}
    ),
}


def load_metadata() -> Metadata:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "cargo metadata failed"
        raise RuntimeError(detail)
    return json.loads(result.stdout)


def internal_edges(metadata: Metadata) -> tuple[set[str], set[tuple[str, str]]]:
    packages_by_id = {package["id"]: package for package in metadata["packages"]}
    workspace_packages_by_path = {
        Path(packages_by_id[package_id]["manifest_path"]).parent.resolve(): (
            packages_by_id[package_id]["name"]
        )
        for package_id in metadata["workspace_members"]
    }
    edges: set[tuple[str, str]] = set()
    for package_id in metadata["workspace_members"]:
        package = packages_by_id[package_id]
        for dependency in package["dependencies"]:
            path = dependency.get("path")
            if path is None:
                continue
            target = workspace_packages_by_path.get(Path(path).resolve())
            if target is not None:
                edges.add((package["name"], target))
    return set(workspace_packages_by_path.values()), edges


def print_items(title: str, items: set[str] | set[tuple[str, str]]) -> None:
    if not items:
        return
    print(f"{title}:", file=sys.stderr)
    for item in sorted(items):
        if isinstance(item, tuple):
            print(f"  {item[0]} -> {item[1]}", file=sys.stderr)
        else:
            print(f"  {item}", file=sys.stderr)


def main() -> int:
    try:
        metadata = load_metadata()
    except (OSError, RuntimeError, json.JSONDecodeError) as e:
        print(f"architecture check failed: {e}", file=sys.stderr)
        return 1

    packages, actual = internal_edges(metadata)
    unexpected_packages = packages - ALLOWED_DEPENDENCIES.keys()
    unexpected_edges = {
        (consumer, dependency)
        for consumer, dependency in actual
        if consumer in ALLOWED_DEPENDENCIES
        and dependency not in ALLOWED_DEPENDENCIES[consumer]
    }

    if unexpected_packages or unexpected_edges:
        print("architecture check failed", file=sys.stderr)
        print_items("workspace packages without dependency policy", unexpected_packages)
        print_items("forbidden internal dependencies", unexpected_edges)
        return 1

    print(
        f"architecture check passed: {len(packages)} packages, {len(actual)} permitted internal edges"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
