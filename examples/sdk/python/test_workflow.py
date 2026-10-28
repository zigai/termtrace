"""Ordinary pytest workflow: guarded input, assertion, reviewed snapshot, retained evidence.

Run with `pytest examples/sdk/python/test_workflow.py`. `TERMTRACE_WORKFLOW_MISMATCH=1` makes the
line test expect a value the fixture never prints; it then fails and reports its evidence.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
from pathlib import Path

from termtrace.testing import (
    ExpectationMismatchError,
    assert_screen_text,
    assert_snapshot_file,
    terminal_test,
)

REPOSITORY = Path(__file__).resolve().parents[3]
LINE_FIXTURE = REPOSITORY / "fixtures/programs/terminal/line-command.sh"
SNAPSHOT = REPOSITORY / "examples/sdk/snapshots/line-value.json"
FAILURE_PREFIX = "TERMTRACE_WORKFLOW_FAILURE "


def test_line_workflow(tmp_path: Path) -> None:
    mismatch = bool(os.environ.get("TERMTRACE_WORKFLOW_MISMATCH"))
    expected = "LINE_VALUE=goodbye" if mismatch else "LINE_VALUE=hello"
    root = os.environ.get("TERMTRACE_WORKFLOW_EVIDENCE", str(tmp_path))

    with terminal_test(
        binary=os.environ.get("TERMTRACE_BIN"),
        evidence_root=root,
        evidence_policy="always",
    ) as ctx:
        rec = ctx.record([str(LINE_FIXTURE)], rows=24, cols=80)
        ready = rec.screen().wait_for_text("LINE_READY", timeout_secs=5.0)
        result = (
            rec.actions()
            .if_state(ready.hash)
            .send_text("hello")
            .send_key("enter")
            .until_screen("LINE_VALUE=", timeout_secs=5.0)
            .run()
        )
        frame = result.frame
        assert frame is not None
        try:
            assert_screen_text(frame, expected)
        except ExpectationMismatchError as error:
            failure = {
                "assertion": str(error),
                "session": rec.path,
                "observation": frame.text,
                "evidence": ctx.evidence_root,
            }
            print(FAILURE_PREFIX + json.dumps(failure), file=sys.stderr)
            raise

        assert_snapshot_file(
            frame,
            SNAPSHOT,
            profile="text",
            update=bool(os.environ.get("TERMTRACE_UPDATE_SNAPSHOTS")),
        )
        report = ctx.finalize("passed")
        assert ctx.finalize("error") == report

    session = next(
        Path(path)
        for path in report["verified_artifacts"]
        if (Path(path) / "completion.json").is_file()
    )
    completion = json.loads((session / "completion.json").read_text())
    assert completion["exit_code"] == 0


def test_mismatch_fails_with_the_assertion_session_observation_and_evidence(
    tmp_path: Path,
) -> None:
    env = dict(
        os.environ,
        TERMTRACE_WORKFLOW_MISMATCH="1",
        TERMTRACE_WORKFLOW_EVIDENCE=str(tmp_path),
    )
    env.pop("TERMTRACE_UPDATE_SNAPSHOTS", None)
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-p",
            "no:cacheprovider",
            f"{__file__}::test_line_workflow",
        ],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    output = run.stdout + run.stderr
    assert run.returncode == 1, output

    line = next(line for line in output.splitlines() if line.startswith(FAILURE_PREFIX))
    failure = json.loads(line.removeprefix(FAILURE_PREFIX))
    assert "LINE_VALUE=goodbye" in failure["assertion"]
    assert "LINE_VALUE=hello" in failure["observation"]

    evidence = Path(failure["evidence"])
    session = Path(failure["session"])
    assert stat.S_IMODE(evidence.stat().st_mode) == 0o700
    assert session.is_relative_to(evidence)
    completion = json.loads((session / "completion.json").read_text())
    manifest = json.loads((session / "session.json").read_text())
    assert completion["session_id"] == manifest["session_id"]
