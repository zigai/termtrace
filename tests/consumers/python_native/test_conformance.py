"""Public sync/async SDK adapters; shared runner validates evidence independently."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

import pytest
from termtrace import StateGuardFailedError, Termtrace, aio


@pytest.mark.parametrize("language", ["python_sync", "python_async"])
def test_foundation_adapter(language: str) -> None:
    root_env = os.environ.get("SDK_CONFORMANCE_ROOT")
    if not root_env:
        # Standalone pytest remains a real run, not a passing skip.
        import tempfile

        root_env = tempfile.mkdtemp(prefix="sdk-conformance-python-")
    root = Path(root_env) / language
    root.mkdir(parents=True, exist_ok=True)
    repo = Path(__file__).resolve().parents[3]
    fixture = json.loads(
        Path(
            os.environ.get(
                "SDK_CONFORMANCE_FIXTURE",
                repo / "fixtures/conformance/sdk/cases/foundation.json",
            )
        ).read_text()
    )
    command = [*fixture["command"], str(root / "child.pid")]
    binary = os.environ.get("TERMTRACE_BIN", str(repo / "target/debug/termtrace"))
    timeout = fixture["timeout_ms"] / 1000
    if language == "python_sync":
        rec = Termtrace(binary=binary, evidence_root=str(root / "evidence")).record(
            command, rows=fixture["rows"], cols=fixture["cols"]
        )
        try:
            rec.screen().wait_for_text(fixture["text"], timeout_secs=timeout)
            frame = rec.screen().frame()
            with pytest.raises(StateGuardFailedError) as stale:
                rec.actions().if_state(fixture["guard"]).send_text(
                    "FORBIDDEN\n"
                ).execute()
            output, _, _ = rec.read_output(0, 4096)
        except BaseException:
            try:
                rec.stop()
            except Exception as cleanup:
                print(f"Secondary SDK adapter cleanup failure: {cleanup}")
                try:
                    rec.force_close()
                except Exception as secondary:
                    print(f"SDK adapter force-close failure: {secondary}")
            raise
        else:
            rec.stop()
        active = rec.is_active()
    else:

        async def run():
            rec = await aio.AsyncTermtrace(
                binary=binary, evidence_root=str(root / "evidence")
            ).record(command, rows=fixture["rows"], cols=fixture["cols"])
            try:
                await rec.screen().wait_for_text(fixture["text"], timeout_secs=timeout)
                frame = await rec.screen().frame()
                with pytest.raises(StateGuardFailedError) as stale:
                    await (
                        rec.actions()
                        .if_state(fixture["guard"])
                        .send_text("FORBIDDEN\n")
                        .run()
                    )
                output, _, _ = await rec.read_output(0, 4096)
            except BaseException:
                try:
                    await rec.stop()
                except Exception as cleanup:
                    print(f"Secondary SDK adapter cleanup failure: {cleanup}")
                raise
            else:
                await rec.stop()
            return rec, frame, stale, output, await rec.is_active()

        rec, frame, stale, output, active = asyncio.run(run())
    observation = dict(
        version=1,
        language=language,
        case_id="foundation",
        session_path=rec.path,
        rows=frame.rows,
        cols=frame.cols,
        text=frame.text,
        output_hex=output.hex(),
        stale=dict(
            kind=stale.value.kind,
            code=stale.value.code,
            effect=stale.value.effect,
            retry=stale.value.retry,
        ),
        active=active,
    )
    (root / "observation.json").write_text(json.dumps(observation, indent=2))
