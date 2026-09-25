#!/usr/bin/env python3
"""Enforce the crate dependency allowlist and the source fences that keep concerns apart.

Crate edges: every direct internal Cargo dependency must be listed in EXPECTED_EDGES.

Source fences (Rust files under crates/, src/, and tests/; test code is any file under a
`tests/` directory or named `tests.rs`):

1. Session file names are spelled once. The literals in SESSION_FILE_LITERALS may appear only in
   crates/model/src/execution/layout.rs and in test code asserting the on-disk contract.
2. Session paths come from `SessionLayout`, and phase is decided by `termtrace-control`.
   Outside test code: no `.join(layout::NAME)` path construction (use `SessionLayout`) outside
   the layout module; no completion existence probe (`.completion().is_file()`, `.exists()`,
   `.try_exists()`) outside crates/control and the canonical completed-session validator
   (COMPLETION_PROBE_ALLOWED); and no function named `is_completed`, `read_completed`, or
   `observe_phase` over a path outside crates/control. Reading completion evidence through
   `SessionLayout::completion()` for validation is allowed; deciding "is it done yet" is not.
3. No `dead_code` suppression at module level (`#![expect/allow(... dead_code ...)]`) anywhere
   except the verbatim bindgen output wrapper (GENERATED_BINDINGS), and no
   `expect/allow(dead_code)` at all in src/.
4. Retry and mutation-effect advice is typed (`protocol::mutation::{MutationEffect, Retry}`);
   the retired string vocabularies in RETRY_VOCABULARY may not appear as string literals.
5. The tmux adapter (TMUX_ADAPTER) drives sessions through service operations and never calls
   CLI command functions (`run_*`).
6. Hidden helper subcommand names are spelled once, in HELPER_CONTRACTS, which both the helper
   invocations and the CLI's argument parser use.
"""

from __future__ import annotations

import json
import re
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


SOURCE_ROOTS = ("crates", "src", "tests")
LAYOUT_MODULE = "crates/model/src/execution/layout.rs"
SESSION_FILE_LITERALS = (
    "session.json",
    "completion.json",
    "events.jsonl",
    "requests.jsonl",
    "failure.json",
    "recovery.json",
    "derived-cache",
    "artifact-roots.jsonl",
)
COMPLETION_PROBE_ALLOWED = ("crates/control/", "crates/evidence/src/storage/session/validation.rs")
RETRY_VOCABULARY = (
    "safe_new_request",
    "safe_same_request_id",
    "same_request_id_only",
    "inspect_before_retry",
)
TMUX_ADAPTER = "src/cli/tmux/adapter.rs"
HELPER_CONTRACTS = ("crates/media/src/helper.rs", "crates/process/src/owned.rs")
HELPER_SUBCOMMAND_LITERAL = re.compile(r'"(playback|playback-worker|geometry-probe|owned-process)"')
# Verbatim rust-bindgen output of the pinned Ghostty C API: it declares the whole API, and only
# the adapter's subset is called.
GENERATED_BINDINGS = ("crates/terminal/src/ghostty_vt/sys.rs",)

SESSION_FILE_LITERAL = re.compile(
    r'"(' + "|".join(re.escape(name) for name in SESSION_FILE_LITERALS) + r')"'
)
LAYOUT_JOIN = re.compile(r"\.join\(\s*layout::\w+\s*\)")
COMPLETION_PROBE = re.compile(r"\.completion\(\)\s*\.\s*(is_file|exists|try_exists)\s*\(")
PHASE_PROBE_DEFINITION = re.compile(
    r"\bfn\s+(is_completed|read_completed|observe_phase)\s*(<[^>]*>)?\s*\(\s*(?!&?\s*(mut\s+)?self\b)"
)
MODULE_DEAD_CODE = re.compile(r"#!\[\s*(expect|allow)\s*\([^\]]*\bdead_code\b")
DEAD_CODE = re.compile(r"\b(expect|allow)\s*\([^)]*\bdead_code\b")
RETRY_LITERAL = re.compile(r'"(' + "|".join(RETRY_VOCABULARY) + r')"')
CLI_COMMAND_CALL = re.compile(r"\brun_\w+")


def is_test_code(path: str) -> bool:
    parts = path.split("/")
    return "tests" in parts[:-1] or parts[-1] == "tests.rs"


def code_lines(text: str) -> str:
    """Blank whole-line comments so prose does not trip literal fences; keep line numbers."""
    return "\n".join(
        "" if line.lstrip().startswith("//") else line for line in text.split("\n")
    )


def line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def rust_sources() -> list[tuple[str, str]]:
    sources: list[tuple[str, str]] = []
    for root in SOURCE_ROOTS:
        for path in sorted((ROOT / root).rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if "/target/" in f"/{relative}" or "/node_modules/" in f"/{relative}":
                continue
            sources.append((relative, path.read_text(encoding="utf-8")))
    return sources


def source_violations() -> list[str]:
    violations: list[str] = []

    def report(path: str, text: str, offset: int, rule: str) -> None:
        violations.append(f"{path}:{line_of(text, offset)}: {rule}")

    for path, raw in rust_sources():
        code = code_lines(raw)
        test = is_test_code(path)

        for match in MODULE_DEAD_CODE.finditer(raw if path not in GENERATED_BINDINGS else ""):
            report(path, raw, match.start(), "module-level dead_code suppression; delete the dead code")
        if path.startswith("src/"):
            for match in DEAD_CODE.finditer(raw):
                report(path, raw, match.start(), "dead_code suppression in the CLI; delete the dead code")
        for match in RETRY_LITERAL.finditer(code):
            report(
                path,
                code,
                match.start(),
                f"retry vocabulary string {match.group(0)}; use protocol::mutation::{{MutationEffect, Retry}}",
            )

        if test or path == LAYOUT_MODULE:
            continue

        for match in SESSION_FILE_LITERAL.finditer(code):
            report(
                path,
                code,
                match.start(),
                f"session file name {match.group(0)} spelled outside {LAYOUT_MODULE}; use a layout constant or SessionLayout",
            )
        for match in LAYOUT_JOIN.finditer(code):
            report(path, code, match.start(), "session path built by joining a layout name; use SessionLayout")
        if not path.startswith(COMPLETION_PROBE_ALLOWED):
            for match in COMPLETION_PROBE.finditer(code):
                report(
                    path,
                    code,
                    match.start(),
                    "completion existence probe; decide phase with termtrace_control::{observe_phase, read_completed, is_completed}",
                )
        if not path.startswith("crates/control/"):
            for match in PHASE_PROBE_DEFINITION.finditer(code):
                report(
                    path,
                    code,
                    match.start(),
                    f"local phase probe `{match.group(1)}`; phase is decided only by termtrace-control",
                )
        if path not in HELPER_CONTRACTS:
            for match in HELPER_SUBCOMMAND_LITERAL.finditer(code):
                report(
                    path,
                    code,
                    match.start(),
                    f"helper subcommand {match.group(0)} spelled outside {HELPER_CONTRACTS}; use its contract constant",
                )
        if path == TMUX_ADAPTER:
            for match in CLI_COMMAND_CALL.finditer(code):
                report(
                    path,
                    code,
                    match.start(),
                    f"tmux adapter calls CLI command function `{match.group(0)}`; use a service operation",
                )

    return violations


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

    violations = source_violations()

    if (
        missing_packages
        or unexpected_packages
        or missing_edges
        or unexpected_edges
        or violations
    ):
        print("architecture check failed", file=sys.stderr)
        print_items("missing workspace packages", missing_packages)
        print_items(
            "unexpected workspace packages (placeholder crates are not permitted)",
            unexpected_packages,
        )
        print_items("missing internal edges", missing_edges)
        print_items("unexpected internal edges", unexpected_edges)
        print_items("source fence violations", set(violations))
        return 1

    print(
        f"architecture check passed: {len(packages)} packages, {len(actual)} internal edges, "
        "source fences clean"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
