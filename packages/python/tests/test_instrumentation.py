from __future__ import annotations

import os
from pathlib import Path

import msgspec
import pytest

from termtrace import AsyncTerminal, Terminal


def recorder() -> Path:
    return Path(
        os.environ.get(
            "TERMTRACE_BINARY", Path(__file__).resolve().parents[3] / "target/debug/termtrace"
        )
    )


def test_application_spans_are_canonical(tmp_path: Path) -> None:
    with Terminal(binary_path=recorder(), evidence_root=tmp_path) as terminal:
        with terminal.launch(["/bin/sh", "-c", "printf READY; read -r answer"]) as session:
            session.output.wait_for_text("READY")
            span = session.application.start_span("work", {"count": 2})
            session.application.event("progress", {"nested": {"ok": True}})
            session.application.end_span("work", span)
            records = [
                msgspec.to_builtins(record.event)["data"]
                for record in session.events.page().events
                if record.event_type == "application"
            ]
            assert [record["kind"] for record in records] == ["span_start", "event", "span_end"]
            assert [record["span_id"] for record in records] == [span, None, span]
            assert all(
                record["process_instance_id"] == session.application.process_instance_id
                for record in records
            )
            assert records[1]["attributes"] == {"nested": {"ok": True}}
            session.keyboard.type("done\n")
            assert session.complete(timeout_ms=10_000).exit_code == 0
            assert session.evidence.verify().valid


@pytest.mark.asyncio
async def test_async_application_spans_are_awaited(tmp_path: Path) -> None:
    async with await AsyncTerminal.create(
        binary_path=recorder(), evidence_root=tmp_path
    ) as terminal:
        async with await terminal.launch(
            ["/bin/sh", "-c", "printf READY; read -r answer"]
        ) as session:
            await session.output.wait_for_text("READY")
            span = await session.application.start_span("work")
            await session.application.event("progress")
            await session.application.end_span("work", span)
            page = await session.events.page()
            assert (
                len([record for record in page.events if record.event_type == "application"]) == 3
            )
            await session.keyboard.type("done\n")
            assert (await session.complete(timeout_ms=10_000)).exit_code == 0
            assert (await session.evidence.verify()).valid
