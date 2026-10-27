#!/usr/bin/env python3
"""Single SDK conformance engine. Adapters make SDK calls, never verdicts.

Reports are separate from scenario/JUnit verdicts. Pending target vectors are
inventory, not passing skips. Native commands share a configurable verification lock.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures/conformance/sdk"
LANGUAGES = ("rust", "python_sync", "python_async", "node")


def inventory(*, check_rendered: bool = True) -> tuple[dict, list[dict]]:
    matrix = json.loads((FIXTURES / "matrix.json").read_text())
    cases = json.loads((FIXTURES / "cases/pending.json").read_text())["cases"]
    ids = {"foundation", "portable"}
    negatives = json.loads((FIXTURES / "cases/portable-invalid.json").read_text())[
        "cases"
    ]
    for negative in negatives:
        case_id = "portable." + negative["id"]
        if case_id in ids:
            raise ValueError(f"duplicate fixture ID: {case_id}")
        ids.add(case_id)
    for case in cases:
        if case["id"] in ids:
            raise ValueError(f"duplicate fixture ID: {case['id']}")
        ids.add(case["id"])
    rows = set()
    for row in matrix["rows"]:
        if row["id"] in rows or not re.fullmatch(r"[a-z_]+\.[a-z_]+", row["id"]):
            raise ValueError(f"duplicate/invalid row ID: {row['id']}")
        rows.add(row["id"])
        if set(row["languages"]) != {"rust", "python", "typescript"}:
            raise ValueError(f"missing language: {row['id']}")
        if (
            not (ROOT / row["core"]["path"]).is_file()
            or not row["core"]["symbol"]
            or not row["canonical_model"]
        ):
            raise ValueError(f"stale source reference: {row['id']}")
        if (
            row["closure_owner"] not in {f"{n:02}" for n in range(1, 12)}
            or not row["availability"]
            or not row["rust_feature"]
            or not row["units"]
        ):
            raise ValueError(f"incomplete ownership/requirements: {row['id']}")
        if (
            set(row["cases"]) != {"positive", "negative", "lifecycle"}
            or not set(row["cases"].values()) <= ids
        ):
            raise ValueError(f"stale fixture reference: {row['id']}")
        for language in row["languages"].values():
            if language["status"] not in {
                "verified",
                "exposed_unverified",
                "partial",
                "missing",
            }:
                raise ValueError("invalid implementation status")
            if row.get("reserved") and language["status"] == "verified":
                raise ValueError("foundation cannot certify reserved media rows")
    by_id = {case["id"]: case for case in cases}
    for row in matrix["rows"]:
        for kind, case_id in row["cases"].items():
            if case_id == "foundation":
                if not row["foundation_required"] or row.get("reserved"):
                    raise ValueError("non-foundation row assigned to foundation case")
            elif (
                case_id not in by_id
                or by_id[case_id]["row_id"] != row["id"]
                or by_id[case_id]["kind"] != kind
            ):
                raise ValueError(f"fixture belongs to another row/kind: {row['id']}")
    document = ROOT / "docs/sdk-capability-contract.md"
    if check_rendered and document.exists():
        rendered = (
            document.read_text()
            .split("<!-- sdk-matrix:start -->\n", 1)[1]
            .split("\n<!-- sdk-matrix:end -->", 1)[0]
        )
        if rendered != table(matrix):
            raise ValueError("rendered SDK matrix is stale")
    if any(c["row_id"] not in rows for c in cases):
        raise ValueError("unknown row ID in fixture")
    return matrix, cases


def table(matrix: dict) -> str:
    result = [
        "| Capability | Rust | Python | TypeScript | Owner |",
        "|---|---|---|---|---|",
    ]
    for row in matrix["rows"]:
        result.append(
            "| "
            + " | ".join(
                [
                    row["id"],
                    *[
                        row["languages"][lang]["status"]
                        for lang in ("rust", "python", "typescript")
                    ],
                    row["closure_owner"],
                ]
            )
            + " |"
        )
    return "\n".join(result) + "\n"


def verification_lock(env: dict[str, str]) -> Path:
    """Select a repo-local default; relative overrides are resolved from ROOT."""
    configured = env.get("TERMTRACE_VERIFICATION_LOCK")
    path = Path(configured) if configured else Path("target/sdk-conformance.lock")
    return path if path.is_absolute() else ROOT / path


def command(
    argv: list[str], directory: Path, env: dict[str, str], *, native: bool = False
) -> dict:
    directory.mkdir(parents=True, exist_ok=True)
    actual = argv
    start = time.monotonic()
    try:
        if native:
            lock = verification_lock(env)
            actual = ["flock", "--close", str(lock), *argv]
            lock.parent.mkdir(parents=True, exist_ok=True)
        process = subprocess.Popen(
            actual,
            cwd=ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            stdout, _ = process.communicate(
                timeout=float(env.get("SDK_CONFORMANCE_TIMEOUT_SECONDS", "600"))
            )
            result = {
                "command": shlex.join(actual),
                "exit_code": process.returncode,
                "execution": "executed",
                "output": stdout,
            }
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                stdout, _ = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, _ = process.communicate()
            result = {
                "command": shlex.join(actual),
                "exit_code": None,
                "execution": "timeout",
                "output": stdout + "\nconformance command timed out",
            }
    except OSError as error:
        result = {
            "command": shlex.join(actual),
            "exit_code": None,
            "execution": "unavailable",
            "output": str(error),
        }
    result["duration_seconds"] = round(time.monotonic() - start, 3)
    (directory / "command.log").write_text(result["output"])
    result["log"] = str(directory / "command.log")
    del result["output"]
    return result


def validate_observation(root: Path, language: str, fixture: dict) -> dict:
    path = root / "observation.json"
    v = json.loads(path.read_text())
    if v["version"] != 1 or v["case_id"] != "foundation" or v["language"] != language:
        raise ValueError("wrong adapter/case/version")
    if (v["rows"], v["cols"]) != (fixture["rows"], fixture["cols"]) or fixture[
        "text"
    ] not in v["text"]:
        raise ValueError("geometry/text mismatch")
    if v["output_hex"] != fixture["output_hex"] or v["active"] is not False:
        raise ValueError("exact output/lifecycle mismatch")
    if v["stale"] != {
        "kind": "stale_state",
        "code": "stale_observation",
        "effect": {"kind": "not_applied"},
        "retry": "new_request",
    }:
        raise ValueError("stale effect/retry mismatch")
    session = Path(v["session_path"]).resolve()
    if not session.is_relative_to((root / "evidence").resolve()):
        raise ValueError("session is not adapter-owned evidence")
    completion = json.loads((session / "completion.json").read_text())
    if not isinstance(completion, dict) or not completion:
        raise ValueError("missing completion evidence")
    events = [
        json.loads(line) for line in (session / "events.jsonl").read_text().splitlines()
    ]
    manifest = json.loads((session / "session.json").read_text())
    records = events[1:]
    if (
        not records
        or events[0].get("session_id") != manifest.get("session_id")
        or completion.get("session_id") != manifest.get("session_id")
    ):
        raise ValueError("completion/source identity mismatch")
    if (
        completion.get("event_count") != len(records)
        or completion.get("final_event_hash") != records[-1].get("hash")
        or completion.get("event_header_hash") != records[0].get("prev_hash")
    ):
        raise ValueError("completion/source boundary mismatch")
    if (
        completion.get("output_bytes") != len(bytes.fromhex(fixture["output_hex"]))
        or completion.get("failure") is not None
    ):
        raise ValueError("completion did not seal expected output")
    output = b""
    for record in events:
        event = record.get("event", {})
        if event.get("type") == "output":
            output += base64.b64decode(event["data"], validate=True)
        if event.get("type") in {"input", "input_intent", "input_result"}:
            raise ValueError("stale guard emitted input")
    if output.hex() != fixture["output_hex"]:
        raise ValueError("canonical exact-output mismatch or forged summary")
    pid = int((root / "child.pid").read_text())
    if pid <= 1 or Path(f"/proc/{pid}").exists():
        raise ValueError("child is still alive/unreaped")
    recorder_pid = int((root / "child.pid.recorder").read_text())
    if recorder_pid <= 1 or Path(f"/proc/{recorder_pid}").exists():
        raise ValueError("recorder is still alive/unreaped")
    return {
        "recorder_pid": recorder_pid,
        "observation": str(path),
        "session": str(session),
        "completion": str(session / "completion.json"),
        "child_pid": pid,
    }


def main() -> int:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument(
        "--profile", choices=("foundation", "inventory"), default="foundation"
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument("--render-table", type=Path)
    parser.add_argument(
        "--without-adapter",
        choices=LANGUAGES,
        action="append",
        default=[],
        help="fault injection: required adapter absence must fail",
    )
    args = parser.parse_args()
    matrix, pending = inventory(check_rendered=not bool(args.render_table))
    if args.render_table:
        args.render_table.write_text(table(matrix))
    if args.profile == "inventory":
        print(
            f"validated {len(matrix['rows'])} leaf rows and {len(pending)} explicitly pending target vectors"
        )
        return 0
    if not sys.platform.startswith("linux"):
        raise ValueError("foundation is supported only on Linux/Ghostty")
    output = (
        args.output or ROOT / "target/sdk-conformance" / time.strftime("%Y%m%d-%H%M%S")
    ).resolve()
    # Refuse reusing evidence: stale adapter output must never turn a missing run green.
    output.mkdir(parents=True, exist_ok=False)
    env = dict(
        os.environ,
        CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "4"),
        CARGO_TARGET_DIR=str(ROOT / "target"),
        TERMTRACE_BIN=os.environ.get(
            "TERMTRACE_BIN", str(ROOT / "target/debug/termtrace")
        ),
        SDK_CONFORMANCE_FIXTURE=str(FIXTURES / "cases/foundation.json"),
    )
    fixture = json.loads(Path(env["SDK_CONFORMANCE_FIXTURE"]).read_text())
    python = str(ROOT / "tests/consumers/python_native/.venv/bin/python")
    adapters = {
        "rust": [
            "cargo",
            "run",
            "--locked",
            "--quiet",
            "--manifest-path",
            "tests/consumers/rust/Cargo.toml",
            "--features",
            "local",
            "--bin",
            "conformance",
        ],
        "python_sync": [
            python,
            "-m",
            "pytest",
            "-q",
            "tests/consumers/python_native/test_conformance.py",
            "-k",
            "python_sync",
        ],
        "python_async": [
            python,
            "-m",
            "pytest",
            "-q",
            "tests/consumers/python_native/test_conformance.py",
            "-k",
            "python_async",
        ],
        "node": [
            "node",
            "--test",
            "tests/consumers/node_native/test/conformance.test.js",
        ],
    }
    report = {
        "version": 1,
        "profile": "foundation",
        "target": "Linux/Ghostty",
        "commands": [],
        "entries": [],
        "checks": [],
        "pending": [
            {
                "row_id": row["id"],
                "owner": row["closure_owner"],
                "reason": "outside foundation verification",
            }
            for row in matrix["rows"]
            if not row["foundation_required"]
        ],
    }
    report["tool_versions"] = {}
    for name, argv in {
        "rustc": ["rustc", "--version"],
        "cargo": ["cargo", "--version"],
        "termtrace": [env["TERMTRACE_BIN"], "--version"],
        "npm": ["npm", "--version"],
        "uv": ["uv", "--version"],
        "node": ["node", "--version"],
        "python": [python, "--version"],
        "typescript": [
            str(ROOT / "tests/consumers/node_native/node_modules/.bin/tsc"),
            "--version",
        ],
    }.items():
        try:
            report["tool_versions"][name] = subprocess.check_output(
                argv, text=True, timeout=10
            ).strip()
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
            report["tool_versions"][name] = str(e)
    failed = False
    for language, argv in adapters.items():
        directory = output / language
        directory.mkdir()
        adapter_env = dict(
            env,
            SDK_CONFORMANCE_ROOT=str(
                output if language.startswith("python_") else directory
            ),
        )
        if language in args.without_adapter:
            execution = {
                "execution": "missing",
                "exit_code": None,
                "command": None,
                "diagnostic": "adapter deliberately unavailable",
            }
        else:
            execution = command(argv, directory, adapter_env, native=True)
        report["commands"].append(execution)
        evidence = None
        diagnostic = None
        try:
            if execution["execution"] != "executed" or execution["exit_code"] != 0:
                raise ValueError("required adapter did not execute successfully")
            evidence = validate_observation(directory, language, fixture)
        except (
            ValueError,
            OSError,
            KeyError,
            TypeError,
            AttributeError,
            IndexError,
        ) as error:
            diagnostic = str(error)
            failed = True
        for row in matrix["rows"]:
            if row["foundation_required"]:
                lang = (
                    "typescript"
                    if language == "node"
                    else "python"
                    if language.startswith("python_")
                    else "rust"
                )
                report["entries"].append(
                    {
                        "row_id": row["id"],
                        "case_id": "foundation",
                        "language": language,
                        "implementation_status": row["languages"][lang]["status"],
                        "execution": execution["execution"],
                        "availability": "available"
                        if execution["exit_code"] == 0
                        else "unavailable"
                        if execution["execution"] in ("missing", "unavailable")
                        else "unknown",
                        "verdict": "failed" if diagnostic else "verified",
                        "observed_result": str(directory / "observation.json")
                        if (directory / "observation.json").is_file()
                        else None,
                        "evidence": evidence
                        if evidence
                        else {
                            "unvalidated_root": str(directory),
                            "command_log": execution.get("log"),
                        },
                        "availability_diagnostics": diagnostic,
                    }
                )
    checks = [
        (
            "testing_shared_rust",
            ["cargo", "test", "--locked", "-p", "termtrace-service", "--test", "assertion_helpers"],
            True,
        ),
        (
            "testing_python",
            [python, "-m", "pytest", "-q", "tests/consumers/python_native/test_testing_helpers.py"],
            True,
        ),
        (
            "testing_node",
            ["node", "--test", "tests/consumers/node_native/test/testing_helpers.test.js"],
            True,
        ),
        (
            "testing_vitest",
            [str(ROOT / "tests/consumers/node_native/node_modules/.bin/vitest"), "run", "--root", "tests/consumers/node_native"],
            True,
        ),
        (
            "testing_rust_consumer",
            ["cargo", "run", "--locked", "--quiet", "--manifest-path", "tests/consumers/rust/Cargo.toml", "--features", "local", "--bin", "testing_helpers"],
            True,
        ),
        (
            "rust_contracts",
            ["cargo", "test", "--locked", "-p", "termtrace-service", "sdk_contract"],
            True,
        ),
        (
            "typescript",
            [
                str(ROOT / "tests/consumers/node_native/node_modules/.bin/tsc"),
                "--noEmit",
                "-p",
                "tests/consumers/node_native/tsconfig.json",
            ],
            False,
        ),
        (
            "node_contracts",
            ["node", "--test", "tests/consumers/node_native/test/contracts.test.js"],
            False,
        ),
        (
            "python_contracts",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/consumers/python_native/test_contracts.py",
                "tests/consumers/python_native/test_async_wrappers.py",
            ],
            True,
        ),
        (
            "node_cleanup",
            ["node", "--test", "crates/bindings-node/test/testing.test.js"],
            False,
        ),
        (
            "conformance_engine",
            [
                "python3",
                "-m",
                "unittest",
                "discover",
                "-s",
                "scripts/tests",
                "-p",
                "test_sdk_conformance.py",
            ],
            False,
        ),
        (
            "python_typing",
            [
                "uv",
                "run",
                "--project",
                "crates/bindings-python",
                "python",
                "scripts/sdk-python-types.py",
            ],
            False,
        ),
    ]
    for name, argv, native in checks:
        result = command(argv, output / name, env, native=native)
        report["commands"].append(result)
        passed = result["execution"] == "executed" and result["exit_code"] == 0
        failed |= not passed
        report["checks"].append(
            {
                "check_id": name,
                "language": name,
                "execution": result["execution"],
                "availability": "available" if passed else "unknown",
                "verdict": "verified" if passed else "failed",
                "observed_result": result["log"],
                "evidence": result["log"],
                "availability_diagnostics": None if passed else "required check failed",
            }
        )
    report["verdict"] = "failed" if failed else "verified_foundation_only"
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"{report['verdict']}: {output / 'report.json'}")
    return int(failed)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError, KeyError) as error:
        print(f"conformance error: {error}", file=sys.stderr)
        sys.exit(1)
