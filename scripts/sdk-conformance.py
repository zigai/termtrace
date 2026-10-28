#!/usr/bin/env python3
"""Single SDK conformance engine. Adapters make SDK calls, never verdicts.

Reports are separate from scenario/JUnit verdicts. Pending target vectors are
inventory, not passing skips. Native commands share a configurable verification lock.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ElementTree

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures/conformance/sdk"
LANGUAGES = ("rust", "python_sync", "python_async", "node")
CONTRACT_FILES = (
    "docs/sdk-capability-contract.md",
    "crates/sdk/src/lib.rs",
    "crates/bindings-python/python/termtrace/_termtrace.pyi",
    "crates/bindings-python/python/termtrace/types.py",
    "crates/bindings-python/python/termtrace/editing.py",
    "crates/bindings-python/python/termtrace/testing.py",
    "crates/bindings-node/index.d.ts",
    "crates/bindings-node/types.d.ts",
    "crates/bindings-node/edit.d.ts",
    "crates/bindings-node/testing.d.ts",
)
EVIDENCE_PREFIXES = ("cargo:", "bin:", "pytest:", "node:", "demo:")


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
                "not_applicable",
            }:
                raise ValueError("invalid implementation status")
            if row.get("reserved") and language["status"] == "verified":
                raise ValueError("foundation cannot certify reserved media rows")
            evidence = language.get("evidence", [])
            if any(not ref.startswith(EVIDENCE_PREFIXES) for ref in evidence):
                raise ValueError(f"unknown evidence reference: {row['id']}")
            if language["status"] == "verified" and not evidence:
                raise ValueError(f"verified without evidence: {row['id']}")
            if language["status"] == "not_applicable" and (
                evidence or not language.get("reason")
            ):
                raise ValueError(
                    f"not_applicable needs a reason and no evidence: {row['id']}"
                )
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


def contract_hash() -> str:
    digest = hashlib.sha256()
    for name in CONTRACT_FILES:
        digest.update(name.encode() + b"\0" + (ROOT / name).read_bytes() + b"\0")
    return digest.hexdigest()


def combine(previous: str | None, outcome: str) -> str:
    order = ("passed", "skipped", "failed")
    return outcome if previous is None else max(previous, outcome, key=order.index)


def junit_results(path: Path, kind: str) -> dict[str, str]:
    """Map `file::test` to passed/skipped/failed; parametrized cases fold into one ID."""
    results: dict[str, str] = {}
    for case in ElementTree.parse(path).iter("testcase"):
        name = re.sub(r"\[.*\]$", "", case.get("name", ""))
        if kind == "pytest":
            parts = case.get("classname", "").split(".")
            index = max(i for i, part in enumerate(parts) if part.startswith("test"))
            test_id = "::".join([parts[index] + ".py", *parts[index + 1 :], name])
        else:
            test_id = Path(case.get("file", "")).name + "::" + name
        if case.find("skipped") is not None:
            outcome = "skipped"
        elif case.find("failure") is not None or case.find("error") is not None:
            outcome = "failed"
        else:
            outcome = "passed"
        results[f"{kind}:{test_id}"] = combine(
            results.get(f"{kind}:{test_id}"), outcome
        )
    return results


def libtest_results(output: str, package: str) -> dict[str, str]:
    """Map `cargo:package/target::test` from libtest's plain output."""
    outcomes = {"ok": "passed", "FAILED": "failed", "ignored": "skipped"}
    results: dict[str, str] = {}
    target = None
    pending = None
    for line in output.splitlines():
        running = re.match(
            r"\s*Running (?:unittests )?(?:src/(lib|main)|tests/(\w+))\.rs", line
        )
        if running:
            target = running.group(1) or running.group(2)
            pending = None
            continue
        started = re.match(r"test (\S+) \.\.\. ?(.*)$", line)
        if started and target:
            name, rest = started.groups()
            if rest in outcomes:
                results[f"cargo:{package}/{target}::{name}"] = outcomes[rest]
                pending = None
            else:
                # Output printed by the test itself pushes the verdict onto a later line.
                pending = name
        elif pending and target and line.strip() in outcomes:
            results[f"cargo:{package}/{target}::{pending}"] = outcomes[line.strip()]
            pending = None
    return results


def validate_mismatch(result: dict, output: str) -> dict:
    """A deliberate mismatch must fail with the assertion, session, observation and evidence."""
    if result["execution"] != "executed" or result["exit_code"] in (0, None):
        raise ValueError("the mismatch run did not fail")
    lines = [
        line.split("TERMTRACE_WORKFLOW_FAILURE ", 1)[1]
        for line in output.splitlines()
        if "TERMTRACE_WORKFLOW_FAILURE " in line
    ]
    if not lines:
        raise ValueError("the mismatch run printed no failure report")
    failure = json.loads(lines[-1])
    if "LINE_VALUE=goodbye" not in failure["assertion"]:
        raise ValueError("the failure does not name the failed assertion")
    if "LINE_VALUE=hello" not in failure["observation"]:
        raise ValueError("the failure does not carry the last observation")
    evidence = Path(failure["evidence"])
    session = Path(failure["session"])
    if evidence.stat().st_mode & 0o777 != 0o700 or not session.is_relative_to(evidence):
        raise ValueError(
            "the evidence directory is not private or does not hold the session"
        )
    completion = json.loads((session / "completion.json").read_text())
    manifest = json.loads((session / "session.json").read_text())
    if completion["session_id"] != manifest["session_id"]:
        raise ValueError("retained completion belongs to another session")
    return failure


def evaluate_cells(matrix: dict, results: dict[str, str]) -> list[dict]:
    """A verified cell certifies only when every referenced test passed in this run."""
    cells = []
    for row in matrix["rows"]:
        for language, entry in row["languages"].items():
            evidence = [
                {"ref": ref, "outcome": results.get(ref, "not_executed")}
                for ref in entry.get("evidence", [])
            ]
            if entry["status"] == "not_applicable":
                verdict = "not_applicable"
            elif entry["status"] == "verified" and all(
                item["outcome"] == "passed" for item in evidence
            ):
                verdict = "verified"
            else:
                verdict = "not_certified"
            cells.append(
                {
                    "row_id": row["id"],
                    "language": language,
                    "status": entry["status"],
                    "evidence": evidence,
                    "verdict": verdict,
                }
            )
    return cells


def certify(args: argparse.Namespace, matrix: dict) -> int:
    output = (
        args.output
        or ROOT / "target/sdk-certification" / time.strftime("%Y%m%d-%H%M%S")
    ).resolve()
    output.mkdir(parents=True, exist_ok=False)
    # Unix socket paths must stay short regardless of the output location, as in ci-run.sh.
    scratch = Path(tempfile.mkdtemp(prefix="ttc.", dir="/tmp"))
    (output / "scratch").symlink_to(scratch)
    termtrace = os.environ.get("TERMTRACE_BIN", str(ROOT / "target/debug/termtrace"))
    env = dict(
        os.environ,
        CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "4"),
        CARGO_TARGET_DIR=str(ROOT / "target"),
        TERMTRACE_BIN=termtrace,
        TMPDIR=str(scratch),
    )
    python = str(ROOT / "tests/consumers/python_native/.venv/bin/python")
    node_tests = sorted(
        str(path)
        for pattern in (
            "tests/consumers/node_native/test/*.test.js",
            "tests/consumers/node_native/.examples/*.test.js",
            "crates/bindings-node/test/*.test.js",
        )
        for path in ROOT.glob(pattern)
    )
    consumer = [
        "cargo",
        "run",
        "--locked",
        "--quiet",
        "--manifest-path",
        "tests/consumers/rust/Cargo.toml",
        "--features",
        "local",
        "--bin",
    ]
    report: dict = {
        "version": 1,
        "profile": "certify",
        "target": "Linux/Ghostty",
        "commands": [],
        "checks": [],
    }
    results: dict[str, str] = {}
    failed_checks: list[str] = []

    def run(
        name: str,
        argv: list[str],
        extra: dict[str, str] | None = None,
        *,
        native: bool = True,
    ) -> tuple[dict, str]:
        result = command(argv, output / name, dict(env, **(extra or {})), native=native)
        report["commands"].append(dict(result, name=name))
        return result, Path(result["log"]).read_text()

    def check(name: str, passed: bool, detail: str | None = None) -> None:
        report["checks"].append(
            {
                "check_id": name,
                "verdict": "verified" if passed else "failed",
                "diagnostic": detail,
            }
        )
        if not passed:
            failed_checks.append(name)

    recorded = matrix.get("contract_sha256")
    actual = contract_hash()
    check(
        "contract_hash",
        recorded == actual,
        None if recorded == actual else f"recorded {recorded}, current {actual}",
    )

    foundation, _ = run(
        "foundation",
        [
            sys.executable,
            str(ROOT / "scripts/sdk-conformance.py"),
            "--profile",
            "foundation",
            "--output",
            str(output / "foundation-report"),
        ],
        native=False,
    )
    passed = foundation["exit_code"] == 0
    check("foundation", passed, None if passed else "foundation profile failed")
    results["bin:conformance"] = "passed" if passed else "failed"

    tsc, _ = run(
        "typescript_examples",
        [
            str(ROOT / "tests/consumers/node_native/node_modules/.bin/tsc"),
            "-p",
            "tests/consumers/node_native/tsconfig.examples.json",
        ],
    )
    check(
        "typescript_examples",
        tsc["exit_code"] == 0,
        "TypeScript examples do not type-check",
    )

    suites = [
        (
            "pytest_consumers",
            "pytest",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "-p",
                "no:cacheprovider",
                "--basetemp",
                str(scratch / "pc"),
                "--junitxml",
                str(output / "pytest-consumers.xml"),
                "tests/consumers/python_native",
            ],
            output / "pytest-consumers.xml",
        ),
        (
            "pytest_examples",
            "pytest",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "-p",
                "no:cacheprovider",
                "--basetemp",
                str(scratch / "pe"),
                "--junitxml",
                str(output / "pytest-examples.xml"),
                "examples/sdk/python",
            ],
            output / "pytest-examples.xml",
        ),
        (
            "node",
            "node",
            [
                "node",
                "--test",
                "--test-reporter=spec",
                "--test-reporter-destination=stdout",
                "--test-reporter=junit",
                f"--test-reporter-destination={output / 'node.xml'}",
                *node_tests,
            ],
            output / "node.xml",
        ),
    ]
    for name, kind, argv, junit in suites:
        result, _ = run(name, argv)
        check(
            name,
            result["exit_code"] == 0,
            None if result["exit_code"] == 0 else "suite failed",
        )
        if junit.is_file():
            results.update(junit_results(junit, kind))

    for name, package, argv in [
        (
            "cargo_service",
            "termtrace-service",
            [
                "cargo",
                "test",
                "--locked",
                "-p",
                "termtrace-service",
                "--lib",
                "--tests",
                "--",
                "--include-ignored",
                "--test-threads=1",
            ],
        ),
        (
            "cargo_scenarios",
            "termtrace",
            [
                "cargo",
                "test",
                "--locked",
                "--test",
                "cli",
                "scenario_and_reviewer_workflows",
            ],
        ),
        (
            "cargo_consumer",
            "termtrace-rust-consumer",
            [
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                "tests/consumers/rust/Cargo.toml",
                "--features",
                "local",
                "--tests",
                "--",
                "--include-ignored",
                "--test-threads=1",
            ],
        ),
    ]:
        result, log = run(name, argv)
        check(
            name,
            result["exit_code"] == 0,
            None if result["exit_code"] == 0 else "suite failed",
        )
        results.update(libtest_results(log, package))

    for binary, arguments, extra in [
        ("local", [str(scratch / "bin-local")], None),
        (
            "testing_helpers",
            [],
            {"TASK04_EVIDENCE": str(scratch / "bin-testing-helpers")},
        ),
        ("diagnostics_evidence", [str(scratch / "bin-diagnostics-evidence")], None),
        ("original", [str(scratch / "bin-original")], None),
        ("editing", [str(scratch / "bin-editing")], None),
    ]:
        result, _ = run(f"bin_{binary}", [*consumer, binary, "--", *arguments], extra)
        results[f"bin:{binary}"] = "passed" if result["exit_code"] == 0 else "failed"
        check(
            f"bin_{binary}",
            result["exit_code"] == 0,
            None if result["exit_code"] == 0 else "consumer failed",
        )

    demos, _ = run(
        "demos", [str(ROOT / "scripts/sdk-demos.sh"), str(output / "demos-run")]
    )
    for language in ("rust", "python", "typescript"):
        results[f"demo:{language}"] = "passed" if demos["exit_code"] == 0 else "failed"
    check(
        "native_demos",
        demos["exit_code"] == 0,
        None
        if demos["exit_code"] == 0
        else "native demos failed or their tools are unavailable",
    )

    mismatch = {"TERMTRACE_WORKFLOW_MISMATCH": "1"}
    for language in ("rust", "python", "typescript"):
        (scratch / f"mismatch-{language}").mkdir()
    for language, argv in [
        ("rust", [*consumer, "workflow", "--", str(scratch / "mismatch-rust")]),
        (
            "python",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "-p",
                "no:cacheprovider",
                "examples/sdk/python/test_workflow.py::test_line_workflow",
            ],
        ),
        (
            "typescript",
            [
                "node",
                "--test-name-pattern=^line workflow$",
                "tests/consumers/node_native/.examples/workflow.test.js",
            ],
        ),
    ]:
        result, log = run(
            f"mismatch_{language}",
            argv,
            dict(
                mismatch,
                TERMTRACE_WORKFLOW_EVIDENCE=str(scratch / f"mismatch-{language}"),
            ),
        )
        try:
            validate_mismatch(result, log)
            check(f"mismatch_{language}", True)
        except (ValueError, OSError, KeyError, TypeError) as error:
            check(f"mismatch_{language}", False, str(error))

    report["cells"] = evaluate_cells(matrix, results)
    uncertified = [
        cell for cell in report["cells"] if cell["verdict"] == "not_certified"
    ]
    report["uncertified"] = [
        {
            "row_id": cell["row_id"],
            "language": cell["language"],
            "status": cell["status"],
        }
        for cell in uncertified
    ]
    report["failed_checks"] = failed_checks
    report["verdict"] = (
        "certified" if not uncertified and not failed_checks else "not_certified"
    )
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        f"{report['verdict']}: {len(uncertified)} uncertified cells, failed checks {failed_checks}: {output / 'report.json'}"
    )
    if report["verdict"] != "certified":
        print(f"scratch kept for inspection: {scratch}")
        return 1
    shutil.rmtree(scratch)
    (output / "scratch").unlink()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument(
        "--profile",
        choices=("foundation", "inventory", "certify"),
        default="foundation",
    )
    parser.add_argument(
        "--record-contract-hash",
        action="store_true",
        help="record the current public contract hash after reviewing the matrix",
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
    if args.record_contract_hash:
        matrix["contract_sha256"] = contract_hash()
        (FIXTURES / "matrix.json").write_text(
            json.dumps(matrix, indent=2, ensure_ascii=False) + "\n"
        )
        print(f"recorded contract hash {matrix['contract_sha256']}")
        return 0
    if args.profile == "certify":
        return certify(args, matrix)
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
