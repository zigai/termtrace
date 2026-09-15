#!/usr/bin/env python3
"""Enforce the complete allowlist of direct internal Cargo dependencies."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import TypedDict


class Dependency(TypedDict):
    name: str


class Package(TypedDict):
    id: str
    name: str
    dependencies: list[Dependency]


class Metadata(TypedDict):
    packages: list[Package]
    workspace_members: list[str]


ROOT = Path(__file__).resolve().parent.parent
EXPECTED_EDGES: dict[str, frozenset[str]] = {
    "termtrace": frozenset(
        {
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-render",
            "termtrace-scenario",
            "termtrace-schema",
            "termtrace-sdk",
            "termtrace-service",
            "termtrace-terminal",
        }
    ),
    "termtrace-engine": frozenset(
        {"termtrace-evidence", "termtrace-model", "termtrace-terminal"}
    ),
    "termtrace-evidence": frozenset({"termtrace-model"}),
    "termtrace-export": frozenset(
        {"termtrace-engine", "termtrace-evidence", "termtrace-model", "termtrace-render"}
    ),
    "termtrace-media": frozenset(
        {
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-render",
            "termtrace-terminal",
        }
    ),
    "termtrace-model": frozenset(),
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
    "termtrace-schema": frozenset({"termtrace-model"}),
    "termtrace-sdk": frozenset(
        {
            "termtrace-media",
            "termtrace-model",
            "termtrace-scenario",
            "termtrace-service",
        }
    ),
    "termtrace-service": frozenset(
        {
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-render",
            "termtrace-terminal",
        }
    ),
    "termtrace-terminal": frozenset({"termtrace-model"}),
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
    workspace_packages = {
        packages_by_id[package_id]["name"] for package_id in metadata["workspace_members"]
    }
    edges: set[tuple[str, str]] = set()
    for package_id in metadata["workspace_members"]:
        package = packages_by_id[package_id]
        for dependency in package["dependencies"]:
            if dependency["name"] in workspace_packages:
                edges.add((package["name"], dependency["name"]))
    return workspace_packages, edges


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
    expected_packages = set(EXPECTED_EDGES)
    expected = {
        (consumer, dependency)
        for consumer, dependencies in EXPECTED_EDGES.items()
        for dependency in dependencies
    }
    missing_packages = expected_packages - packages
    unexpected_packages = packages - expected_packages
    missing_edges = expected - actual
    unexpected_edges = actual - expected

    if missing_packages or unexpected_packages or missing_edges or unexpected_edges:
        print("architecture check failed", file=sys.stderr)
        print_items("missing workspace packages", missing_packages)
        print_items(
            "unexpected workspace packages (placeholder crates are not permitted)",
            unexpected_packages,
        )
        print_items("missing internal edges", missing_edges)
        print_items("unexpected internal edges", unexpected_edges)
        return 1

    print(
        f"architecture check passed: {len(packages)} packages, {len(actual)} internal edges"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
