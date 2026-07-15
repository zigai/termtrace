#!/usr/bin/env python3
"""Generate the skill's concise command reference from tested CLI help."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


COMMANDS = (
    "start",
    "status",
    "send",
    "wait",
    "show",
    "output",
    "events",
    "mark",
    "timeline",
    "diff",
    "explain",
    "diagnose",
    "screenshot",
    "stop",
    "verify",
    "bundle",
    "test",
    "review",
    "markers",
    "video",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    return parser.parse_args()


def command_help(binary: Path, command: str) -> str:
    completed = subprocess.run(
        (str(binary), command, "--help"),
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return completed.stdout.rstrip()


def main() -> None:
    arguments = parse_args()
    sections = [
        "# Generated Termtrace command reference",
        "",
        "Generated from the current tested command graph. Prefer `termtrace COMMAND --help` when using a different binary version.",
    ]
    for command in COMMANDS:
        sections.extend(("", f"## `{command}`", "", "```text", command_help(arguments.binary, command), "```"))
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    arguments.out.write_text("\n".join(sections) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
