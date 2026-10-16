"""End-to-end integration tests for Termtrace native Python SDK (100% API coverage)."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import tempfile
from typing import Any
from unittest.mock import Mock

import pytest
from termtrace import (
    Recording,
    StateGuardFailedError,
    Termtrace,
    TermtraceError,
    TimeoutError,
    aio,
)
from termtrace.aio import AsyncTermtrace

TERMTRACE_BIN = os.path.abspath(
    os.environ.get("TERMTRACE_BIN", "target/debug/termtrace")
)


def test_sync_recording_lifecycle() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo PY_SYNC_OK"], rows=24, cols=80) as rec:
            assert rec.is_active()
            assert len(rec.session_id) > 0
            screen = rec.screen()
            state = screen.wait_for_text("PY_SYNC_OK", timeout_secs=5.0)
            assert "PY_SYNC_OK" in state.text
            assert state.cursor_row >= 0
        assert not rec.is_active()


def test_screen_frame_and_text() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo FRAME_TEST_LINE"], rows=30, cols=100) as rec:
            screen = rec.screen()
            screen.wait_for_text("FRAME_TEST_LINE", timeout_secs=5.0)
            frame = screen.frame()
            assert frame.rows == 30
            assert frame.cols == 100
            assert "FRAME_TEST_LINE" in frame.text
            plain_text = screen.text()
            assert "FRAME_TEST_LINE" in plain_text


def test_action_builder_state_guarded_input() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc"], rows=24, cols=80) as rec:
            screen = rec.screen()
            state = screen.wait_for_text("$", timeout_secs=5.0)

            actions = rec.actions()
            actions.if_state(state.hash)
            actions.send_text("echo ACTION_DISPATCHED")
            actions.send_key("Enter")
            actions.until_screen("ACTION_DISPATCHED", timeout_secs=5.0)
            outcome = actions.execute()
            assert outcome is not None
            assert "ACTION_DISPATCHED" in outcome.text


def test_pointer_actions() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc"], rows=24, cols=80) as rec:
            rec.screen().wait_for_text("$", timeout_secs=5.0)
            # Send pointer click, scroll, and focus
            rec.pointer_click(column=10, row=5, button="left")
            rec.pointer_scroll(column=10, row=5, lines=2)
            rec.pointer_focus(focused=True)


def test_visual_snapshot_save() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo VISUAL_SAVE_TEST"], rows=24, cols=80) as rec:
            rec.screen().wait_for_text("VISUAL_SAVE_TEST", timeout_secs=5.0)
            save_dir = os.path.join(tmpdir, "snapshot_out")
            manifest_json = rec.save(save_dir, formats=["png", "svg", "html", "txt", "json"])
            manifest = json.loads(manifest_json)
            assert "format" in manifest
            assert os.path.isdir(save_dir)
            assert "files" in manifest
            assert len(manifest["files"]) > 0
            first_file = os.path.join(save_dir, manifest["files"][0]["path"])
            assert os.path.isfile(first_file)


def test_raw_output_streaming() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo OUTPUT_STREAM_LINE"], rows=24, cols=80) as rec:
            rec.screen().wait_for_text("OUTPUT_STREAM_LINE", timeout_secs=5.0)
            # Read output bytes
            raw_bytes, next_off, has_more = rec.read_output(offset=0, limit=4096)
            assert len(raw_bytes) > 0
            assert b"OUTPUT_STREAM_LINE" in raw_bytes
            assert next_off > 0
            # Wait for output bytes
            matched = rec.wait_for_output("OUTPUT_STREAM_LINE", timeout_secs=5.0)
            assert matched


def test_event_log_reading() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo EVENT_TEST"], rows=24, cols=80) as rec:
            rec.screen().wait_for_text("EVENT_TEST", timeout_secs=5.0)
            events_json = rec.read_events(cursor=0, limit=50)
            events = json.loads(events_json)
            assert isinstance(events, list)
            assert len(events) > 0


def test_replay_analysis_services() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc"], rows=24, cols=80) as rec:
            state = rec.screen().wait_for_text("$", timeout_secs=5.0)
            rec.actions().send_text("echo ACTION_FOR_ANALYSIS").send_key("Enter").until_screen("ACTION_FOR_ANALYSIS").execute()
            session = rec.session()
            state_hash = state.hash

        # Once recording has finalized and completion.json is sealed:
        explanation = json.loads(session.explain(target_hash=state_hash, context=1))
        assert "target" in explanation
        diagnosis = json.loads(session.diagnose(broad_change_percent=20))
        assert "findings" in diagnosis
        last_act = json.loads(session.last_action())
        assert isinstance(last_act, dict)
        timeline = json.loads(session.timeline_page(offset=0, limit=10))
        assert "entries" in timeline


def test_system_capabilities() -> None:
    tt = Termtrace(binary=TERMTRACE_BIN)
    caps_json = tt.probe_capabilities()
    caps = json.loads(caps_json)
    assert "capabilities" in caps
    assert isinstance(caps["capabilities"], list)


def test_registry_operations() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo REGISTRY_TEST"], rows=24, cols=80) as rec:
            session_path = rec.path
            alias = f"test-alias-{os.getpid()}"
            # Register
            entry = json.loads(tt.registry_register(alias, session_path))
            assert entry["alias"] == alias
            # Resolve
            resolved = tt.registry_resolve(alias)
            assert resolved == session_path
            # List
            entries = json.loads(tt.registry_list(include_all=True))
            assert any(e["alias"] == alias for e in entries)
            # Remove
            removed = tt.registry_remove(alias)
            assert removed
            # GC (dry run)
            gc_report = json.loads(tt.registry_gc(older_than_days=1, apply=False))
            assert "state_root" in gc_report


def test_session_integrity_verification() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo INTEGRITY_TEST"], rows=24, cols=80) as rec:
            session_path = rec.path
        # Once closed, verify structure
        report = json.loads(tt.verify_session(session_path, level="structure"))
        assert "valid" in report
        assert report["valid"] is True


@pytest.mark.asyncio
async def test_async_recording_lifecycle() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        async with await tt.record(["/bin/sh", "-norc", "-c", "echo PY_ASYNC_OK"]) as rec:
            assert rec.is_active()
            screen = rec.screen()
            state = await screen.wait_for_text("PY_ASYNC_OK", timeout=5.0)
            assert "PY_ASYNC_OK" in state.text
        assert not rec.is_active()


@pytest.mark.asyncio
@pytest.mark.parametrize("completed", [False, True], ids=["pending", "completed"])
@pytest.mark.parametrize(
    "failure",
    [None, "force_close", "stop", "launch"],
    ids=["closed", "fallback", "cleanup-failed", "launch-failed"],
)
async def test_cancelled_recording_launch_is_cleaned_up_and_failures_reported(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
    completed: bool,
    failure: str | None,
) -> None:
    loop = asyncio.get_running_loop()
    launched: asyncio.Future[Recording] = loop.create_future()
    started = asyncio.Event()
    cleaned = asyncio.Event()
    rec = Mock(spec=Recording)
    rec.force_close.side_effect = cleaned.set
    if failure in {"force_close", "stop"}:
        rec.force_close.side_effect = TermtraceError("force close failed")
    if failure == "stop":
        rec.stop.side_effect = TermtraceError("stop failed")

    async def launch_in_thread(*args: Any, **kwargs: Any) -> Recording:
        started.set()
        return await launched

    def observe_cleanup(record: logging.LogRecord) -> None:
        cleaned.set()

    handler = logging.Handler()
    monkeypatch.setattr(handler, "emit", observe_cleanup)
    logger = logging.getLogger("termtrace.aio")
    logger.addHandler(handler)
    caplog.set_level(logging.ERROR, logger="termtrace.aio")
    monkeypatch.setattr(aio.asyncio, "to_thread", launch_in_thread)
    tt = AsyncTermtrace(binary=TERMTRACE_BIN)
    pending_before = set(aio._pending_launches)
    task = asyncio.create_task(tt.record(["/bin/sh"]))
    try:
        await asyncio.wait_for(started.wait(), 1.0)
        assert len(aio._pending_launches - pending_before) == 1
        if completed:
            if failure == "launch":
                launched.set_exception(TermtraceError("launch failed"))
            else:
                launched.set_result(rec)
            loop.call_soon(task.cancel)
        else:
            task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        if not completed:
            assert len(aio._pending_launches - pending_before) == 1
            if failure == "launch":
                launched.set_exception(TermtraceError("launch failed"))
            else:
                launched.set_result(rec)
        await asyncio.wait_for(cleaned.wait(), 1.0)
        if failure == "launch":
            rec.force_close.assert_not_called()
        else:
            rec.force_close.assert_called_once_with()
        if failure in {"force_close", "stop"}:
            rec.stop.assert_called_once_with()
        else:
            rec.stop.assert_not_called()
        records = [record for record in caplog.records if record.name == "termtrace.aio"]
        assert len(records) == {
            None: 0,
            "force_close": 1,
            "stop": 2,
            "launch": 1,
        }[failure]
        assert all(record.exc_info is not None for record in records)
        assert aio._pending_launches == pending_before
    finally:
        logger.removeHandler(handler)
        if not launched.done():
            launched.cancel()
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


def test_asciicast_export() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "echo ASCIICAST_TEST"], rows=24, cols=80) as rec:
            screen = rec.screen()
            screen.wait_for_text("ASCIICAST_TEST", timeout_secs=5.0)
            session = rec.session()
            cast_out = os.path.join(tmpdir, "playback.cast")
            session.export_asciicast(cast_out)
            assert os.path.isfile(cast_out)
            assert os.path.getsize(cast_out) > 0


def test_pytest_plugin_fixture(termtrace_session: Any) -> None:
    rec = termtrace_session(["/bin/sh", "-norc", "-c", "echo PYTEST_FIXTURE_OK"], rows=24, cols=80)
    screen = rec.screen()
    state = screen.wait_for_text("PYTEST_FIXTURE_OK", timeout_secs=5.0)
    assert "PYTEST_FIXTURE_OK" in state.text


def test_timeout_error_raised() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc", "-c", "sleep 10"], rows=24, cols=80) as rec:
            screen = rec.screen()
            with pytest.raises(TimeoutError) as raised:
                screen.wait_for_text("NON_EXISTENT_STRING_XYZ", timeout_secs=0.2)
            assert raised.value.kind == "wait_unsatisfied"
            assert raised.value.effect == {"kind": "not_applied"}
            assert raised.value.retry == "new_request"

            guarded = rec.actions().if_state("0" * 64).send_key("ctrl+shift+F5")
            with pytest.raises(StateGuardFailedError) as stale:
                guarded.execute()
            assert stale.value.kind == "stale_state"
            assert stale.value.code == "stale_observation"
            assert stale.value.effect == {"kind": "not_applied"}
            assert stale.value.retry == "new_request"
