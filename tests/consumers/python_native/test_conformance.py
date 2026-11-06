"""Foundation conformance case shared with the Rust and Node consumer suites."""

from __future__ import annotations

import asyncio
import json
import os
import traceback
from pathlib import Path

import pytest
from termtrace import StateGuardFailedError, Termtrace, aio

REPO = Path(__file__).resolve().parents[3]
FIXTURE = json.loads(
    (REPO / "fixtures/conformance/sdk/cases/foundation.json").read_text()
)
BINARY = os.environ.get("TERMTRACE_BIN", str(REPO / "target/debug/termtrace"))
STALE = {
    "kind": "stale_state",
    "code": "stale_observation",
    "effect": {"kind": "not_applied"},
    "retry": "new_request",
}


def run_sync(command: list[str], evidence: Path):
    rec = Termtrace(binary=BINARY, evidence_root=str(evidence)).record(
        command, rows=FIXTURE["rows"], cols=FIXTURE["cols"]
    )
    try:
        rec.screen().wait_for_text(
            FIXTURE["text"], timeout_secs=FIXTURE["timeout_ms"] / 1000
        )
        frame = rec.screen().frame()
        with pytest.raises(StateGuardFailedError) as stale:
            rec.actions().if_state(FIXTURE["guard"]).send_text("FORBIDDEN\n").execute()
        output, _, _ = rec.read_output(0, 4096)
    except BaseException:
        try:
            rec.stop()
        except Exception as cleanup:  # noqa: BLE001 - best-effort cleanup; the original error is re-raised
            print(f"Secondary SDK cleanup failure: {cleanup}")
            traceback.print_exc()
            try:
                rec.force_close()
            except Exception as secondary:  # noqa: BLE001 - best-effort cleanup; the original error is re-raised
                print(f"SDK force-close failure: {secondary}")
                traceback.print_exc()
        raise
    rec.stop()
    return rec.path, frame, stale.value, output, rec.is_active()


async def run_async(command: list[str], evidence: Path):
    rec = await aio.AsyncTermtrace(binary=BINARY, evidence_root=str(evidence)).record(
        command, rows=FIXTURE["rows"], cols=FIXTURE["cols"]
    )
    try:
        await rec.screen().wait_for_text(
            FIXTURE["text"], timeout_secs=FIXTURE["timeout_ms"] / 1000
        )
        frame = await rec.screen().frame()
        with pytest.raises(StateGuardFailedError) as stale:
            await (
                rec.actions().if_state(FIXTURE["guard"]).send_text("FORBIDDEN\n").run()
            )
        output, _, _ = await rec.read_output(0, 4096)
    except BaseException:
        try:
            await rec.stop()
        except Exception as cleanup:  # noqa: BLE001 - best-effort cleanup; the original error is re-raised
            print(f"Secondary SDK cleanup failure: {cleanup}")
            traceback.print_exc()
        raise
    await rec.stop()
    return rec.path, frame, stale.value, output, await rec.is_active()


def process_is_gone(pid_file: Path) -> bool:
    pid = int(pid_file.read_text())
    return pid > 1 and not Path(f"/proc/{pid}").exists()


@pytest.mark.parametrize("mode", ["sync", "async"])
def test_foundation_case_matches_the_shared_fixture(mode: str, tmp_path: Path) -> None:
    command = [*FIXTURE["command"], str(tmp_path / "child.pid")]
    evidence = tmp_path / "evidence"
    if mode == "sync":
        session, frame, stale, output, active = run_sync(command, evidence)
    else:
        session, frame, stale, output, active = asyncio.run(
            run_async(command, evidence)
        )

    assert (frame.rows, frame.cols) == (FIXTURE["rows"], FIXTURE["cols"])
    assert FIXTURE["text"] in frame.text
    assert {
        "kind": stale.kind,
        "code": stale.code,
        "effect": stale.effect,
        "retry": stale.retry,
    } == STALE
    assert output.hex() == FIXTURE["output_hex"]
    assert active is False

    session_dir = Path(session)
    completion = json.loads((session_dir / "completion.json").read_text())
    assert completion["output_bytes"] == len(output)
    assert completion.get("failure") is None
    records = (session_dir / "events.jsonl").read_text().splitlines()[1:]
    kinds = {json.loads(line).get("event", {}).get("type") for line in records}
    assert not kinds & {"input", "input_intent", "input_result"}
    assert process_is_gone(tmp_path / "child.pid")
    assert process_is_gone(tmp_path / "child.pid.recorder")
