"""Source-bound shared paging and exact native codecs, using actual public facades."""

from __future__ import annotations

import json
import os
import shutil
import uuid
from pathlib import Path

import pytest
from termtrace import Events, Output, Termtrace, TermtraceError, aio, types

ROOT = Path(__file__).resolve().parents[3]
BINARY = os.environ.get("TERMTRACE_BIN", str(ROOT / "target/debug/termtrace"))


def evidence_root(tmp_path: Path) -> Path:
    root = Path(os.environ.get("TASK05_EVIDENCE", str(tmp_path))) / str(uuid.uuid4())
    root.mkdir(parents=True)
    return root


def test_cursor_json_round_trips_exact_integers_and_path_bytes() -> None:
    events = Events(b"/unused")
    cursor: types.EventPageCursor = {
        "event_hash": "b" * 64,
        "next_event_seq": 2**64 - 1,
        "source": {
            "device": 2**64 - 1,
            "header_hash": "a" * 64,
            "inode": 2**53 + 1,
            "session_id": "00000000-0000-0000-0000-000000000000",
            "session_path": b"/tmp/nonutf8-\xff",
            "session_path_display": "/tmp/nonutf8-\ufffd",
        },
    }
    text = events.cursor_to_json(cursor)
    wire = json.loads(text)
    assert wire["next_event_seq"] == 2**64 - 1
    assert wire["source"]["inode"] == 2**53 + 1
    assert events.cursor_from_json(text) == cursor
    exact = '"next_event_seq":18446744073709551615'
    assert exact in text
    for replacement in ['"18446744073709551615"', "18446744073709551616", "-1", "1.5", "01"]:
        with pytest.raises(ValueError):
            events.cursor_from_json(
                text.replace(exact, f'"next_event_seq":{replacement}')
            )
    output = Output(b"/unused")
    value: types.OutputPageCursor = {"events": cursor, "byte_offset": 2**64 - 1}
    encoded = output.cursor_to_json(value)
    assert json.loads(encoded)["byte_offset"] == 2**64 - 1
    assert output.cursor_from_json(encoded) == value


def test_native_pages_resume_after_owned_stop_and_reject_foreign_paths(
    tmp_path: Path,
) -> None:
    root = evidence_root(tmp_path)
    client = Termtrace(binary=BINARY, evidence_root=str(root))
    recording = client.launch(["/bin/sh", "-c", "printf 'A\\000\\377BC'; read line"])
    try:
        output = recording.output()
        assert output.wait_for(b"A\x00\xffBC", timeout_secs=5)["output_end_offset"] == 5
        first = output.page(limit=3)
        assert first["bytes"] == b"A\x00\xff"
        assert first["has_more"]
        assert isinstance(first["cursor"]["events"]["source"]["session_path"], bytes)
        events = recording.events()
        position = events.follow()
        assert position["kind"] == "live"
        assert position["device"] == first["cursor"]["events"]["source"]["device"]
        recording.mark("evidence-page")
        projected = events.page(limit=1, metadata_only=True, event_types=["output"])
        assert projected["events"][0]["event"] is None
        assert projected["events"][0]["byte_payloads"][0]["byte_count"] == 5
        recording.stop()
        second = output.page(first["cursor"], 2)
        assert second["bytes"] == b"BC"
        assert not second["has_more"]
        eof = output.page(second["cursor"], 2)
        assert eof["cursor"] == second["cursor"]
        assert eof["bytes"] == b""
        assert events.follow()["kind"] == "completed"
        pages = []
        cursor = None
        while True:
            page = events.page(cursor, 1)
            pages.extend(page["events"])
            cursor = page["cursor"]
            if not page["has_more"]:
                break
        assert [entry["seq"] for entry in pages] == list(range(len(pages)))
        assert any(
            entry["event"] and entry["event"]["type"] == "marker" for entry in pages
        )
        nonutf8 = os.fsencode(root) + b"/copied-\xff.tt"
        shutil.copytree(os.fsencode(recording.path), nonutf8)
        copied = Output(nonutf8)
        with pytest.raises(TermtraceError, match="cursor belongs"):
            copied.page(first["cursor"], 2)
        exact = copied.page(limit=10)
        assert exact["bytes"] == b"A\x00\xffBC"
        assert exact["cursor"]["events"]["source"]["session_path"] == nonutf8
        for limit in (0, 4194305):
            with pytest.raises(TermtraceError):
                output.page(limit=limit)
        for invalid in (-1, 0.5, 2**64):
            with pytest.raises((TypeError, ValueError, OverflowError)):
                output.page({**first["cursor"], "byte_offset": invalid}, 2)
        (root / "observed.json").write_text(
            json.dumps(
                {
                    "source": recording.path,
                    "output_hex": (first["bytes"] + second["bytes"]).hex(),
                    "event_count": len(pages),
                    "follow": "completed",
                    "nonutf8_path_hex": nonutf8.hex(),
                }
            )
        )
    finally:
        if recording.is_active():
            recording.stop()
    with pytest.raises(ValueError, match="unknown variant `signatur`"):
        client.verify_session(recording.path, "signatur")


@pytest.mark.asyncio
async def test_async_pages_and_follow_use_the_same_native_namespace(
    tmp_path: Path,
) -> None:
    root = evidence_root(tmp_path)
    client = aio.AsyncTermtrace(binary=BINARY, evidence_root=str(root))
    recording = await client.launch(["/bin/sh", "-c", "printf READY; read line"])
    try:
        await recording.output().wait_for(b"READY", 5)
        first = await recording.output().page(limit=2)
        assert first["bytes"] == b"RE"
        assert (await recording.events().follow())["kind"] == "live"
        await recording.stop()
        assert (await recording.output().page(first["cursor"], 3))["bytes"] == b"ADY"
        assert (await recording.events().follow())["kind"] == "completed"
    finally:
        if await recording.is_active():
            await recording.stop()
