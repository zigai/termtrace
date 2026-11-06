"""End-to-end integration tests for Termtrace native Python SDK (100% API coverage)."""

from __future__ import annotations

import asyncio
import itertools
import json
import logging
import math
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path
from fractions import Fraction
from typing import Any
from unittest.mock import Mock

import pytest
from termtrace import (
    ActionResult,
    CancelledError,
    CancelToken,
    Edit,
    LocatorSpec,
    LocatorTimeoutError,
    QueryError,
    Recording,
    Scope,
    ScreenFrame,
    Select,
    StateGuardFailedError,
    Termtrace,
    TermtraceError,
    TimeoutError,
    aio,
    normalize_edit,
    output_frames_for_source_frame,
    source_for_output_frame,
    types,
)
from termtrace import _async
from termtrace.aio import AsyncTermtrace

TERMTRACE_BIN = os.path.abspath(
    os.environ.get("TERMTRACE_BIN", "target/debug/termtrace")
)
LOCATOR_FIXTURES = Path(__file__).resolve().parents[3] / "fixtures/conformance/locators"
PARITY = json.loads((LOCATOR_FIXTURES / "parity.json").read_text(encoding="utf-8"))
PARITY_FRAME_JSON = (LOCATOR_FIXTURES / PARITY["frame"]).read_text(encoding="utf-8")
CAPTURE_QUANTUM_SECS = 0.25
_U64_MAX = 2**64 - 1


def test_sync_recording_lifecycle() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo PY_SYNC_OK"], rows=24, cols=80
        ) as rec:
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
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo FRAME_TEST_LINE"], rows=30, cols=100
        ) as rec:
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
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo VISUAL_SAVE_TEST"], rows=24, cols=80
        ) as rec:
            rec.screen().wait_for_text("VISUAL_SAVE_TEST", timeout_secs=5.0)
            save_dir = os.path.join(tmpdir, "snapshot_out")
            saved = rec.save(save_dir, formats=["png", "svg", "html", "txt", "json"])
            assert saved["directory"] == save_dir
            manifest = saved["manifest"]
            assert "format" in manifest
            assert os.path.isdir(save_dir)
            assert "files" in manifest
            assert len(manifest["files"]) > 0
            first_file = os.path.join(save_dir, manifest["files"][0]["path"])
            assert os.path.isfile(first_file)


def test_raw_output_streaming() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo OUTPUT_STREAM_LINE"], rows=24, cols=80
        ) as rec:
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
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo EVENT_TEST"], rows=24, cols=80
        ) as rec:
            rec.screen().wait_for_text("EVENT_TEST", timeout_secs=5.0)
            events = rec.read_events(cursor=0, limit=50)
            assert isinstance(events, list)
            assert len(events) > 0


def test_replay_analysis_services() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(["/bin/sh", "-norc"], rows=24, cols=80) as rec:
            state = rec.screen().wait_for_text("$", timeout_secs=5.0)
            rec.actions().send_text("echo ACTION_FOR_ANALYSIS").send_key(
                "Enter"
            ).until_screen("ACTION_FOR_ANALYSIS").execute()
            session = rec.session()
            state_hash = state.hash

        # Once recording has finalized and completion.json is sealed:
        explanation = session.explain(target_hash=state_hash, context=1)
        assert "target" in explanation
        diagnosis = session.diagnose(broad_change_percent=20)
        assert "findings" in diagnosis
        last_act = session.last_action()
        assert isinstance(last_act, dict)
        timeline = session.timeline_page(offset=0, limit=10)
        assert "entries" in timeline


def test_system_capabilities() -> None:
    tt = Termtrace(binary=TERMTRACE_BIN)
    caps = tt.probe_capabilities()
    assert "capabilities" in caps
    assert isinstance(caps["capabilities"], list)


def test_registry_operations() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo REGISTRY_TEST"], rows=24, cols=80
        ) as rec:
            session_path = rec.path
            alias = f"test-alias-{os.getpid()}"
            # Register
            entry = tt.registry_register(alias, session_path)
            assert entry["alias"] == alias
            # Resolve
            resolved = tt.registry_resolve(alias)
            assert resolved == session_path
            # List
            entries = tt.registry_list(include_all=True)
            assert any(e["alias"] == alias for e in entries)
            # Remove
            removed = tt.registry_remove(alias)
            assert removed
            # GC (dry run)
            gc_report = tt.registry_gc(older_than_days=1, apply=False)
            assert "state_root" in gc_report


def test_session_integrity_verification() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo INTEGRITY_TEST"], rows=24, cols=80
        ) as rec:
            session_path = rec.path
        # Once closed, verify structure
        report = tt.verify_session(session_path, level="structure")
        assert "valid" in report
        assert report["valid"] is True


@pytest.mark.asyncio
async def test_async_recording_lifecycle() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        async with await tt.record(
            ["/bin/sh", "-norc", "-c", "echo PY_ASYNC_OK"]
        ) as rec:
            assert await rec.is_active()
            screen = rec.screen()
            state = await screen.wait_for_text("PY_ASYNC_OK", timeout_secs=5.0)
            assert "PY_ASYNC_OK" in state.text
            assert await rec.wait_for_output("PY_ASYNC_OK", timeout_secs=5.0)
            output, _, _ = await rec.read_output()
            assert b"PY_ASYNC_OK" in output
            await rec.save(os.path.join(tmpdir, "async-screen"), formats=["txt"])
        assert not await rec.is_active()
        report = await tt.verify_session(rec.path, level="structure")
        assert report["valid"] is True


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
    pending_before = set(_async._pending_launches)
    task = asyncio.create_task(tt.record(["/bin/sh"]))
    try:
        await asyncio.wait_for(started.wait(), 1.0)
        assert len(_async._pending_launches - pending_before) == 1
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
            assert len(_async._pending_launches - pending_before) == 1
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
        records = [
            record for record in caplog.records if record.name == "termtrace.aio"
        ]
        assert (
            len(records)
            == {
                None: 0,
                "force_close": 1,
                "stop": 2,
                "launch": 1,
            }[failure]
        )
        assert all(record.exc_info is not None for record in records)
        assert _async._pending_launches == pending_before
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
        with tt.record(
            ["/bin/sh", "-norc", "-c", "echo ASCIICAST_TEST"], rows=24, cols=80
        ) as rec:
            screen = rec.screen()
            screen.wait_for_text("ASCIICAST_TEST", timeout_secs=5.0)
            session = rec.session()
            cast_out = os.path.join(tmpdir, "playback.cast")
            session.export_asciicast(cast_out)
            assert os.path.isfile(cast_out)
            assert os.path.getsize(cast_out) > 0


def test_pytest_plugin_fixture(termtrace_session: Any) -> None:
    rec = termtrace_session(
        ["/bin/sh", "-norc", "-c", "echo PYTEST_FIXTURE_OK"], rows=24, cols=80
    )
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


def parity_spec(spec: dict[str, Any]) -> LocatorSpec:
    if "text" in spec:
        locator = LocatorSpec.text(spec["text"])
    elif "exact" in spec:
        locator = LocatorSpec.exact(spec["exact"])
    elif "regex" in spec:
        locator = LocatorSpec.regex(spec["regex"])
    else:
        locator = LocatorSpec()
    if "row" in spec:
        locator = locator.in_row(spec["row"])
    if "region" in spec:
        locator = locator.in_region(spec["region"])
    if "style" in spec:
        locator = locator.with_style(spec["style"])
    if spec.get("cross_soft_wraps"):
        locator = locator.across_soft_wraps()
    return locator


def triples(ranges: list[Any]) -> list[list[int]]:
    return [[r["row"], r["start_column"], r["end_column"]] for r in ranges]


def check_parity_case(frame: ScreenFrame, case: dict[str, Any]) -> None:
    name, expect = case["name"], case["expect"]
    try:
        result = frame.query(parity_spec(case["spec"]))
        operation = case.get("operation")
        if operation is None:
            found = result.all()
        elif operation == "nth":
            found = [result.nth(case["index"])]
        else:
            found = [getattr(result, operation)()]
    except QueryError as error:
        assert "error" in expect, f"{name}: {error}"
        expected = expect["error"]
        assert error.kind == expected["kind"], name
        assert error.effect == {"kind": "not_applied"}, name
        for field in ("count", "index", "state_hash"):
            if field in expected:
                assert getattr(error, field) == expected[field], f"{name}: {field}"
        if "ranges" in expected:
            assert triples(error.ranges or []) == expected["ranges"], name
        return

    assert "matches" in expect, f"{name}: unexpected success"
    observed = [
        {
            "text": m.text,
            "ranges": triples(m.ranges),
            "widths": [c["width"] for c in m.cells],
        }
        for m in found
    ]
    assert observed == expect["matches"], name
    assert all(m.frame.frame_hash == frame.frame_hash for m in found), name


def test_offline_parity_corpus_and_exact_frame_facts() -> None:
    frame = ScreenFrame.from_json(PARITY_FRAME_JSON)
    facts = PARITY["frame_facts"]
    assert frame.state_hash == facts["state_hash"]
    assert frame.source["observed_monotonic_ns"] == int(facts["observed_monotonic_ns"])
    assert frame.source["observed_monotonic_ns"] == 2**64 - 1
    observed = frame.source["boundaries"]["observed"]
    assert (
        observed["next_event_seq"] == int(facts["observed_next_event_seq"]) == 2**53 + 1
    )
    assert frame.cursor["row"] is None and frame.cursor["column"] is None
    assert json.loads(frame.to_json()) == json.loads(PARITY_FRAME_JSON)
    assert len(PARITY["cases"]) >= 12
    for case in PARITY["cases"]:
        check_parity_case(frame, case)


def test_locator_inputs_reject_host_regexes_and_bad_shapes() -> None:
    with pytest.raises(TypeError, match="Rust regex syntax"):
        LocatorSpec.regex(re.compile("Save"))  # type: ignore[arg-type]
    for pattern in ["x*", "(", ""]:
        with pytest.raises(QueryError) as invalid:
            LocatorSpec.regex(pattern)
        assert invalid.value.kind == "invalid_query"
    with pytest.raises(ValueError, match="unknown field `bogus`"):
        LocatorSpec().with_style({"bogus": True})  # type: ignore[typeddict-unknown-key]
    with pytest.raises(ValueError, match="missing field `bottom`"):
        LocatorSpec().in_region({"top": 0, "left": 0, "right": 1})  # type: ignore[typeddict-item]
    with pytest.raises(QueryError) as empty:
        LocatorSpec().in_region({"top": 0, "left": 0, "bottom": 0, "right": 1})
    assert empty.value.kind == "invalid_query"
    with pytest.raises(ValueError):
        ScreenFrame.from_json("{}")
    for not_bool in (None, 1, "yes"):
        with pytest.raises((TypeError, ValueError)):
            LocatorSpec().with_style({"bold": not_bool})  # type: ignore[typeddict-item]


def test_live_locator_parity_rich_frame_and_waits() -> None:
    live = PARITY["live"]
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with tt.record(live["command"], rows=live["rows"], cols=live["cols"]) as rec:
            screen = rec.screen()
            ready = screen.get_by_text(live["ready"]).wait(
                timeout_secs=10, interval_secs=0.02
            )
            for case in live["cases"]:
                check_parity_case(ready.frame, case)

            frame = screen.frame()
            assert (frame.rows, frame.cols) == (live["rows"], live["cols"])
            assert isinstance(frame.rows, int) and "END" in frame.text
            assert frame.content["detail"] == "spans"
            assert len(frame.content["rows"]) == live["rows"]
            assert frame.geometry == {"cols": live["cols"], "rows": live["rows"]}
            assert frame.source["source"] == "actor"
            assert isinstance(frame.source["observed_monotonic_ns"], int)
            assert frame.modes["wraparound"] is True and frame.modes["values"]
            assert len(frame.colors["palette"]) == 256
            assert frame.content["rows"][5]["soft_wrapped"] is True
            assert frame.content["rows"][6]["wrap_continuation"] is True

            region = {"top": 1, "left": 0, "bottom": 2, "right": 6}
            cells = screen.frame(detail="cells", region=region)
            assert cells.returned_region == region
            assert cells.content["detail"] == "cells"
            assert [c["wide_kind"] for c in cells.content["rows"][0]["cells"]][:2] == [
                "wide",
                "spacer_tail",
            ]
            with pytest.raises(TermtraceError):
                screen.frame(detail="cells")

            retained = frame.query(LocatorSpec.text("Save"))
            assert retained.frame.frame_hash == frame.frame_hash
            assert retained.count() == 2

            state = screen.wait_for_text("END", timeout_secs=5)
            assert isinstance(state.cursor_row, int)
            assert state.cursor["row"] == state.cursor_row
            assert state.frame.state_hash == state.hash

            assert screen.get_by_text("Save").count() == 2
            assert (
                screen.get_by_text("Save")
                .wait_for({"count": 2}, timeout_secs=5)
                .matches
            )
            stable = screen.get_by_text("Cancel").wait_stable(
                timeout_secs=5, interval_secs=0.02, stable_for_secs=0.05
            )
            assert len(stable.matches) == 1

            unmatched = screen.wait_for(
                {"predicate": "contains", "text": "NEVER-SHOWN"}, timeout_secs=0.3
            )
            assert unmatched["matched"] is False and unmatched["satisfied"] is False
            assert unmatched["quality"] == "deadline"
            assert isinstance(unmatched["frame"], ScreenFrame)
            matched = screen.wait_for(
                {"predicate": "row_contains", "row": 7, "text": "END"}
            )
            assert matched["satisfied"] and isinstance(
                matched["first_match_monotonic_ns"], int
            )

            capture = screen.capture_stable(stable_for_secs=0.05, timeout_secs=5)
            assert capture["quality"] == "stable"
            assert isinstance(capture["semantic_state_zstd"], bytes)
            assert capture["stable_for_ns"] == 50_000_000
            observation = screen.observe()
            assert isinstance(observation["boundary"]["next_event_seq"], int)
            assert observation["state_hash"]

            started = time.monotonic()
            with pytest.raises(LocatorTimeoutError) as timed_out:
                screen.get_by_text("NEVER-SHOWN").wait_for(
                    timeout_secs=1, interval_secs=0.02
                )
            elapsed = time.monotonic() - started
            assert isinstance(timed_out.value, TimeoutError)
            assert timed_out.value.kind == "locator_timeout"
            assert timed_out.value.frame is not None
            assert timed_out.value.effect == {"kind": "not_applied"}
            assert elapsed <= 1 + CAPTURE_QUANTUM_SECS + 0.5

            locator = screen.get_by_text("NEVER-SHOWN")
            for bad in (-1.0, math.nan, math.inf, 3600.5):
                with pytest.raises(ValueError):
                    locator.wait_for(timeout_secs=bad)
            with pytest.raises(QueryError):
                locator.wait_for(timeout_secs=0)
            with pytest.raises(QueryError):
                locator.wait_for(timeout_secs=1, stable_for_secs=2)
            with pytest.raises(ValueError, match="unknown variant `present`"):
                locator.wait_for("present")  # type: ignore[arg-type]

            token = CancelToken()
            outcome: dict[str, Any] = {}

            def wait() -> None:
                try:
                    locator.wait_for(timeout_secs=30, interval_secs=0.02, cancel=token)
                except CancelledError as error:
                    outcome["error"] = error

            waiter = threading.Thread(target=wait)
            waiter.start()
            time.sleep(0.3)
            cancelled_at = time.monotonic()
            token.cancel()
            waiter.join(timeout=1.5)
            latency = time.monotonic() - cancelled_at
            assert not waiter.is_alive(), "cancelled wait did not return"
            assert token.cancelled
            assert outcome["error"].kind == "cancelled"
            assert outcome["error"].frame is not None
            assert latency < 1.5
            print(f"sync python cancellation latency: {latency:.3f}s")
            assert "END" in screen.frame().text
            with pytest.raises(TermtraceError):
                screen.frame_at({"selector": "state_id", "state_id": 0})

        completed = tt.attach(rec.path).screen()
        first = completed.frame_at(
            {"selector": "state_id", "state_id": 0}, include_hidden=True
        )
        assert first.capture_quality == "completed"
        assert first.source["source"] == "resolved"
        assert first.source["state_id"] == 0
        assert first.source["source_completed"] is True
        observed = completed.observe(
            {"selector": "state_id", "state_id": 0}, include_hidden=True
        )
        assert observed["state_id"] == 0 and observed["source_completed"] is True
        assert isinstance(observed["canonical_session"], bytes)
        final = completed.frame()
        assert final.presented and "END" in final.text
        assert completed.get_by_text("Save").count() == 2

        with tt.record(
            ["/bin/sh", "-c", "printf LOADING; sleep 1; printf '\\r\\033[K'; sleep 30"],
            rows=5,
            cols=40,
        ) as erasing:
            loading = erasing.screen().get_by_text("LOADING")
            loading.wait(timeout_secs=5)
            gone = loading.wait_for("absent", timeout_secs=5, interval_secs=0.02)
            assert gone.matches == []
            assert "LOADING" not in gone.frame.text


@pytest.mark.asyncio
async def test_async_locator_cancellation_joins_worker(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    loop = asyncio.get_running_loop()
    workers: list[asyncio.Future[Any]] = []
    run_in_executor = loop.run_in_executor

    def capture(executor: Any, func: Any, *args: Any) -> asyncio.Future[Any]:
        future = run_in_executor(executor, func, *args)
        workers.append(future)
        return future

    with tempfile.TemporaryDirectory() as tmpdir:
        tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        async with await tt.record(["/bin/sh", "-c", "printf READY; sleep 30"]) as rec:
            screen = rec.screen()
            await screen.get_by_text("READY").wait(timeout_secs=10)
            monkeypatch.setattr(loop, "run_in_executor", capture)
            task = asyncio.create_task(
                screen.get_by_text("NEVER-SHOWN").wait_for(
                    timeout_secs=30, interval_secs=0.02
                )
            )
            await asyncio.sleep(0.3)
            cancelled_at = time.monotonic()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            latency = time.monotonic() - cancelled_at
            assert len(workers) == 1 and workers[0].done()
            assert isinstance(workers[0].exception(), CancelledError)
            assert latency < 1.5
            print(f"async python cancellation latency: {latency:.3f}s")
            monkeypatch.undo()
            assert "READY" in (await screen.frame()).text
            assert (await screen.get_by_text("READY").single()).text == "READY"


ACTION_FIXTURES = Path(__file__).resolve().parents[3] / "fixtures/conformance/actions"
INTERACTION = json.loads(
    (ACTION_FIXTURES / "interaction-control.json").read_text(encoding="utf-8")
)


def error_facts(error: TermtraceError) -> dict[str, Any]:
    return {
        "kind": error.kind,
        "code": error.code,
        "effect": error.effect,
        "retry": error.retry,
    }


def send_step_id(send_request_id: str, index: int) -> str:
    name = INTERACTION["send_step_name"].format(send=send_request_id, index=index)
    return str(uuid.uuid5(uuid.UUID(INTERACTION["send_step_namespace"]), name))


def launch_fixture(tt: Termtrace, command_name: str, **options: Any) -> Recording:
    rec = tt.launch(INTERACTION[command_name], options or None)
    rec.screen().wait_for_text(INTERACTION["ready_text"], timeout_secs=5.0)
    return rec


def screen_text(rec: Recording) -> str:
    return rec.screen().text()


def test_run_seals_the_request_and_replays_it_with_the_same_identity() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "echo_command") as rec:
            chosen = uuid.uuid4()
            builder = rec.actions().with_request_id(chosen)
            assert builder.request_id == str(chosen)
            with pytest.raises(TermtraceError) as invalid:
                rec.actions().with_request_id("not-a-uuid")
            assert invalid.value.kind == "invalid_operation"

            builder.send_text("once").send_key("enter").until_screen_where(
                {"predicate": "contains", "text": "GOT:once"}, timeout_ms=5000
            )
            result = builder.run()
            assert builder.sealed
            assert result.request_id == str(chosen)
            assert result.input["sequence_id"] == str(chosen)
            assert result.matched
            assert result.frame is not None
            assert result.state_hash == result.postcondition["state_hash"]

            for change in (
                lambda: builder.send_text("more"),
                lambda: builder.with_request_id(uuid.uuid4()),
                lambda: builder.allow_incomplete(),
                builder.execute,
            ):
                with pytest.raises(TermtraceError) as sealed:
                    change()
                assert error_facts(sealed.value) == INTERACTION["errors"]["sealed"]

            retry = builder.run()
            assert retry.input == result.input
            assert screen_text(rec).count("GOT:once") == 1
            restored = ActionResult.from_json(result.to_json())
            assert restored.input == result.input
            assert restored.postcondition == result.postcondition


def test_legacy_execute_sends_a_fresh_request_each_call() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "echo_command") as rec:
            builder = rec.actions()
            first_id = builder.request_id
            builder.send_text("one\n").until_screen("GOT:one", timeout_secs=5.0)
            state = builder.execute()
            assert state is not None
            assert isinstance(state.cursor_row, int)
            second_id = builder.request_id
            assert second_id != first_id
            assert not builder.sealed
            builder.send_text("two\n").until_screen(
                "GOT:two", timeout_secs=5.0
            ).execute()
            assert builder.request_id not in {first_id, second_id}
            text = screen_text(rec)
            assert text.count("GOT:one") == 1
            assert text.count("GOT:two") == 1


def test_mutation_failures_match_the_shared_fixture() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "echo_command") as rec:
            stale_builder = rec.actions().if_state("0" * 64).send_text("stale\n")
            with pytest.raises(StateGuardFailedError) as stale:
                stale_builder.run()
            assert error_facts(stale.value) == INTERACTION["errors"]["stale"]
            assert stale.value.request_id == stale_builder.request_id
            assert stale.value.details is not None
            assert stale.value.details["boundaries"]["durable"]["next_event_seq"] > 0

            first = rec.actions().send_text("first\n")
            first.run()
            with pytest.raises(TermtraceError) as collision:
                rec.actions().with_request_id(first.request_id).send_text(
                    "other\n"
                ).run()
            assert error_facts(collision.value) == INTERACTION["errors"]["collision"]

            with pytest.raises(TermtraceError) as strict:
                rec.pointer().click(0, 0, strict=True)
            assert error_facts(strict.value) == INTERACTION["errors"]["strict_pointer"]
            assert strict.value.request_id is not None

            late = (
                rec.actions()
                .send_text("late\n")
                .until_screen("NEVER", timeout_secs=0.2)
            )
            with pytest.raises(TermtraceError) as incomplete:
                late.run()
            assert error_facts(incomplete.value) == INTERACTION["errors"]["incomplete"]
            assert isinstance(incomplete.value.result, ActionResult)
            assert incomplete.value.result.input["atom_count"] == 1
            assert not incomplete.value.result.matched

            rec.screen().wait_for_text("GOT:late", timeout_secs=5.0)
            text = screen_text(rec)
            assert "GOT:first" in text
            assert "GOT:stale" not in text
            assert "GOT:other" not in text


def test_send_reports_derived_step_identities_and_partial_failures() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "echo_command") as rec:
            send_id = str(uuid.uuid4())
            result = rec.send(
                [
                    {"step": "mark", "label": "before"},
                    {"step": "input", "atoms": [{"text": "sent"}, {"key": "enter"}]},
                ],
                request_id=send_id,
                until={
                    "condition": "screen",
                    "predicate": {"predicate": "contains", "text": "GOT:sent"},
                },
            )
            assert result["request_id"] == send_id
            last = result["last"]
            assert last["kind"] == "action"
            assert last["step"] == 1
            assert last["request_id"] == send_step_id(send_id, 1)
            assert last["result"].matched

            failing_id = str(uuid.uuid4())
            with pytest.raises(TermtraceError) as failure:
                rec.send(
                    [
                        {"step": "mark", "label": "applied"},
                        {
                            "step": "pointer",
                            "column": 0,
                            "row": 0,
                            "operation": {"kind": "click", "button": "left"},
                            "strict": True,
                        },
                    ],
                    request_id=failing_id,
                )
            assert failure.value.step == 1
            assert failure.value.send_request_id == failing_id
            assert failure.value.request_id == send_step_id(failing_id, 1)
            assert error_facts(failure.value) == INTERACTION["errors"]["strict_pointer"]
            labels = [
                event["label"]
                for event in rec.read_events(0, 1000)
                if event["type"] == "marker"
            ]
            assert labels == ["before", "applied"]


def test_raw_bytes_and_wide_integers_stay_exact() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "raw_command") as rec:
            raw = bytes.fromhex(INTERACTION["raw_bytes_hex"])
            with pytest.raises(ValueError, match="Invalid symbol"):
                rec.send(
                    [{"step": "input", "atoms": [{"bytes": raw.decode("latin-1")}]}]
                )  # type: ignore[typeddict-item]
            result = rec.actions().send_bytes(memoryview(bytearray(raw))).run()
            assert result.input["bytes_written"] == len(raw)
            rec.screen().wait_for_text(INTERACTION["raw_bytes_echo"], timeout_secs=5.0)
            written = [
                event["data"]
                for event in rec.read_events(0, 1000)
                if event["type"] == "input_intent"
            ]
            assert raw in written

    report = json.loads(result.to_json())
    assert "version" not in report
    assert report["input"]["bytes_written"] == len(raw)
    wide = int(INTERACTION["wide_decimal"])
    report["input"]["bytes_written"] = wide
    assert ActionResult.from_json(json.dumps(report)).input["bytes_written"] == wide
    for malformed in [INTERACTION["wide_decimal"], -1, 1.5, 2**64]:
        report["input"]["bytes_written"] = malformed
        with pytest.raises(ValueError):
            ActionResult.from_json(json.dumps(report))


def test_typed_pointer_operations_report_their_outcomes() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "mouse_command") as rec:
            pointer = rec.pointer()
            multi = pointer.multi_click(3, 0, 2, strict=True)
            assert multi["kind"] == "applied"
            assert multi["bytes"].count(b"\x1b[<") == 4
            assert multi["bytes_written"] == len(multi["bytes"])
            drag = pointer.drag(1, 0, 5, 0, steps=3, strict=True)
            assert drag["kind"] == "applied"
            assert b"<32;" in drag["bytes"]
            activated = pointer.activate(
                INTERACTION["activation_text"], unique=True, strict=True
            )
            assert activated["kind"] == "activated"
            assert (activated["text"], activated["start_column"], activated["row"]) == (
                INTERACTION["activation_text"],
                0,
                0,
            )
        with launch_fixture(tt, "echo_command") as rec:
            noop = rec.pointer().click(0, 0)
            assert noop["kind"] == "noop"
            assert noop["reason"] == "pointer_reporting_disabled"
            assert rec.pointer_click(column=0, row=0) is None


def test_session_controls_and_status() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        rec = launch_fixture(tt, "echo_command")
        try:
            status = rec.status()
            assert status["kind"] == "live"
            assert status["boundaries"]["durable"]["next_event_seq"] > 0
            resized = rec.resize(100, 30)
            assert resized["request_id"]
            frame = rec.screen().frame()
            assert (frame.cols, frame.rows) == (100, 30)
            rec.signal("interrupt")
            rec.screen().wait_for_text("SIGNAL:INT", timeout_secs=5.0)
            marked = rec.mark("checkpoint")
            assert marked["label"] == "checkpoint"
            assert marked["event_seq"] > 0
            stop_id = str(uuid.uuid4())
            stopped = rec.request_stop(request_id=stop_id, wait_ms=5000)
            assert stopped["kind"] == "completed"
            assert stopped["request_id"] == stop_id
            assert not rec.is_active()
            assert rec.status()["kind"] == "completed"
        finally:
            if rec.is_active():
                rec.force_close()


def test_launch_environment_is_isolated_and_exact(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TT_COPY", INTERACTION["copy_value"])
    monkeypatch.setenv("TT_SECRET", INTERACTION["secret_value"])
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        rec = tt.launch(
            INTERACTION["environment_command"],
            {
                "environment": {
                    "base": "clear",
                    "set": {"TT_SET": bytes.fromhex(INTERACTION["set_bytes_hex"])},
                    "copy": ["TT_COPY"],
                }
            },
        )
        with rec:
            text = rec.screen().wait_for_text("DONE", timeout_secs=5.0).text
            assert f"SET= {INTERACTION['set_bytes_echo']}" in text
            assert f"COPY={INTERACTION['copy_value']}" in text
            assert "SECRET=absent" in text
            assert re.search(r"TERM=\S+", text)
        secret = INTERACTION["secret_value"].encode()
        for path in Path(tmpdir).rglob("*"):
            if path.is_file():
                assert secret not in path.read_bytes(), path


@pytest.mark.parametrize("limit", ["max_duration", "idle_timeout"])
def test_launch_limits_end_the_session(limit: str) -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        rec = tt.launch(
            ["/bin/sh", "-c", "printf READY; sleep 30"], {f"{limit}_ms": 400}
        )
        completion = rec.wait_for_completion(timeout_ms=10_000)
        assert completion["resource_limit"]["kind"] == limit
        assert completion["resource_limit"]["limit"] == 400
        assert rec.status()["kind"] == "completed"


async def test_async_run_cancellation_keeps_the_admitted_action() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        async with await tt.launch(INTERACTION["echo_command"]) as rec:
            await rec.screen().wait_for_text(
                INTERACTION["ready_text"], timeout_secs=5.0
            )
            builder = (
                rec.actions()
                .send_text("slow\n")
                .until_screen_where(
                    {"predicate": "contains", "text": "NEVER"}, timeout_ms=800
                )
                .allow_incomplete()
            )
            request_id = builder.request_id
            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(builder.run(), 0.05)
            await rec.wait_pending()
            retry = await builder.run()
            assert retry.request_id == request_id
            assert not retry.matched
            assert (await rec.screen().text()).count("GOT:slow") == 1


async def test_async_run_seals_before_scheduling() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        async with await tt.launch(INTERACTION["echo_command"]) as rec:
            await rec.screen().wait_for_text(
                INTERACTION["ready_text"], timeout_secs=5.0
            )
            builder = rec.actions().send_text("sealed\n")
            pending = builder.run()
            with pytest.raises(TermtraceError) as sealed:
                builder.send_text("changed\n")
            assert error_facts(sealed.value) == INTERACTION["errors"]["sealed"]
            task = asyncio.ensure_future(pending)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert "GOT:sealed" not in await rec.screen().text()

            result = await builder.run()
            assert result.request_id == builder.request_id
            await rec.screen().wait_for_text("GOT:sealed", timeout_secs=5.0)
            text = await rec.screen().text()
            assert text.count("GOT:sealed") == 1
            assert "changed" not in text

            send = await rec.send([{"step": "mark", "label": "async"}])
            assert send["last"]["outcome"]["kind"] == "marked"
            noop = await rec.pointer().click(0, 0)
            assert noop["kind"] == "noop"
            assert (await rec.status())["kind"] == "live"


def test_shared_atomic_action_fixture_matches_the_rust_summary() -> None:
    fixture = json.loads(
        (ACTION_FIXTURES / "atomic-action.json").read_text(encoding="utf-8")
    )
    region = fixture["region"]
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        options = {"rows": region["height"], "cols": region["width"]}
        with tt.launch(fixture["command"], options) as rec:
            assert rec.wait_for_output(fixture["ready_text"], timeout_secs=5.0)
            initial = rec.screen().frame()
            builder = (
                rec.actions()
                .with_request_id(fixture["request_id"])
                .if_state(initial.state_hash)
            )
            for atom in fixture["input"]:
                if atom["kind"] == "text":
                    builder.send_text(atom["text"])
                else:
                    builder.send_key(atom["key"])
            builder.until_screen_where(
                fixture["predicate"], timeout_ms=5000
            ).projection(
                fixture["frame_detail"],
                {
                    "top": region["top"],
                    "left": region["left"],
                    "right": region["left"] + region["width"],
                    "bottom": region["top"] + region["height"],
                },
            )
            result = builder.run()
            retry = builder.run()
            stable = rec.screen().capture_stable(stable_for_secs=0.02, timeout_secs=2.0)
            frame = result.frame
            assert frame is not None
            source = frame.source
            summary = {
                "state_hash_valid": len(frame.state_hash) == 64,
                "atom_count": result.input["atom_count"],
                "matched": result.matched
                and result.postcondition.get("quality")
                in {"matched", "matched_unsettled"}
                and fixture["expected_text"] in frame.text,
                "presented": frame.presented,
                "retry_exact": retry.to_json() == result.to_json(),
                "stable_quality": stable["quality"] == "stable",
                "actor_boundaries": source["source"] == "actor"
                and source["boundaries"] == result.boundaries,
            }
    assert summary == {
        "state_hash_valid": True,
        "atom_count": fixture["expected_atom_count"],
        "matched": True,
        "presented": True,
        "retry_exact": True,
        "stable_quality": True,
        "actor_boundaries": True,
    }


def test_nested_frame_wide_fields_are_exact_integers() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        with launch_fixture(tt, "echo_command") as rec:
            result = (
                rec.actions()
                .send_text("framed\n")
                .until_screen_where(
                    {"predicate": "contains", "text": "GOT:framed"}, timeout_ms=5000
                )
                .run()
            )
    report = json.loads(result.to_json())
    source = report["postcondition"]["frame"]["source"]
    assert source["source"] == "actor"
    assert isinstance(source["observed_monotonic_ns"], int)
    assert isinstance(source["boundaries"]["observed"]["next_event_seq"], int)
    for value in (int(INTERACTION["wide_decimal"]), int(INTERACTION["u64_max"])):
        source["observed_monotonic_ns"] = value
        source["boundaries"]["observed"]["next_event_seq"] = value
        decoded = ActionResult.from_json(json.dumps(report))
        assert decoded.frame is not None
        decoded_source = decoded.frame.source
        assert decoded_source["source"] == "actor"
        assert decoded_source["observed_monotonic_ns"] == value
        assert decoded_source["boundaries"]["observed"]["next_event_seq"] == value
        assert json.loads(decoded.to_json()) == report
    for bad in [INTERACTION["u64_max"], "7", -1, 1.5, 2**64]:
        source["observed_monotonic_ns"] = bad
        with pytest.raises(ValueError):
            ActionResult.from_json(json.dumps(report))


def test_rejected_stop_keeps_the_recording_attached() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        rec = launch_fixture(tt, "echo_command")
        try:
            used = str(uuid.uuid4())
            rec.mark("before-stop", request_id=used)
            with pytest.raises(TermtraceError) as rejected:
                rec.request_stop(request_id=used, wait_ms=5000)
            assert error_facts(rejected.value) == INTERACTION["errors"]["collision"]
            assert rejected.value.request_id == used
            assert rec.is_active()
            assert rec.status()["kind"] == "live"
            after = (
                rec.actions()
                .send_text("after\n")
                .until_screen("GOT:after", timeout_secs=5.0)
            )
            assert after.run().matched
            rec.stop()
            assert not rec.is_active()
            assert rec.status()["kind"] == "completed"
        finally:
            if rec.is_active():
                rec.force_close()


VISUAL_PROFILES = json.loads(
    (
        Path(__file__).resolve().parents[3]
        / "fixtures/conformance/sdk/visual-profiles.json"
    ).read_text(encoding="utf-8")
)
SEMANTIC_PROFILE = {"version": 1, "semantic": {"preset": "bundled-v1"}}


def assert_visual_profile_report(
    report: dict[str, Any], vector: dict[str, Any]
) -> None:
    assert report["version"] == 1
    assert report["profile"] == vector["normalized"]
    assert report["profile_hash"] == vector["profile_hash"]
    if vector["native"]:
        native = report["native"]
        assert native["dpi"] == 96
        assert native["discovered_font_families"] == ["JetBrains Mono"]
        assert (
            native["geometry"]["required_cols"],
            native["geometry"]["required_rows"],
        ) == (40, 10)
        assert len(native["effective_config_hash"]) == 64
    else:
        assert report["native"] is None
    if "semantic" in vector["normalized"]:
        assert report["semantic"]["font_set_blake3"]
        assert (
            report["semantic"]["cell_width"],
            report["semantic"]["cell_height"],
        ) == (12, 24)
    else:
        assert report["semantic"] is None


async def test_visual_profile_vectors_agree_in_sync_and_async_preflight() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        sync = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        asynchronous = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)

        for vector in VISUAL_PROFILES["positive"]:
            assert_visual_profile_report(
                sync.preflight_visual_profile(vector["profile"], cols=40, rows=10),
                vector,
            )
            assert_visual_profile_report(
                await asynchronous.preflight_visual_profile(
                    vector["profile"], cols=40, rows=10
                ),
                vector,
            )

        for vector in VISUAL_PROFILES["invalid"]:
            with pytest.raises(TermtraceError) as invalid:
                sync.preflight_visual_profile(vector["profile"])
            assert (invalid.value.kind, invalid.value.code) == (
                "invalid_operation",
                "invalid_visual_profile",
            ), vector["name"]
            with pytest.raises(TermtraceError) as invalid_async:
                await asynchronous.preflight_visual_profile(vector["profile"])
            assert invalid_async.value.code == "invalid_visual_profile", vector["name"]

        for vector in VISUAL_PROFILES["malformed"]:
            with pytest.raises(ValueError, match=vector["message"]):
                sync.preflight_visual_profile(vector["profile"])
            with pytest.raises(ValueError, match=vector["message"]):
                await asynchronous.preflight_visual_profile(vector["profile"])


def test_semantic_visual_profile_records_without_native_tools(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("PATH", "")
    with tempfile.TemporaryDirectory() as tmpdir:
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=tmpdir)
        report = tt.preflight_visual_profile(SEMANTIC_PROFILE)
        assert report["native"] is None
        with tt.record(
            ["/bin/sh", "-c", "echo SEMANTIC_PROFILE"], visual_profile=SEMANTIC_PROFILE
        ) as rec:
            assert rec.wait_for_output("SEMANTIC_PROFILE", timeout_secs=5.0)
        manifest = json.loads(
            (Path(rec.path) / "session.json").read_text(encoding="utf-8")
        )
        assert manifest["version"] == 13
        assert manifest["visual_profile"]["profile_hash"] == report["profile_hash"]
        assert manifest["ghostty_visual"] is None

        with pytest.raises(TermtraceError) as unavailable:
            tt.preflight_visual_profile(VISUAL_PROFILES["positive"][1]["profile"])
        assert unavailable.value.code == "native_profile_unavailable"


def test_native_visual_profile_failures_stop_before_the_application_starts() -> None:
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir) / "evidence"
        marker = Path(tmpdir) / "started"
        tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(root))
        missing_font = {
            "version": 1,
            "native": {"font_family": "Termtrace Missing Family 7c1d"},
        }
        with pytest.raises(TermtraceError) as missing:
            tt.record(
                ["/bin/sh", "-c", 'printf started > "$1"', "probe", str(marker)],
                visual_profile=missing_font,
            )
        assert (missing.value.kind, missing.value.code) == (
            "engine",
            "font_unavailable",
        )

        with pytest.raises(TermtraceError) as invalid:
            tt.launch(
                ["/bin/sh", "-c", 'printf started > "$1"', "probe", str(marker)],
                {"visual_profile": VISUAL_PROFILES["invalid"][2]["profile"]},
            )
        assert invalid.value.code == "invalid_visual_profile"
        assert not marker.exists()
        assert not root.exists() or not any(root.iterdir())


SDK_FIXTURES = (
    Path(__file__).resolve().parents[3] / "fixtures/conformance/sdk"
).resolve()
EDITING = json.loads((SDK_FIXTURES / "editing.json").read_text(encoding="utf-8"))
EDIT_SESSION_SCRIPT = "trap 'exit 0' TERM; i=0; while :; do i=$((i+1)); printf 'line %s\\n' $i; sleep 0.4; done"


def fixture_edit(
    resource_base: Path | None, subtitle: Path | str = "editing-captions.ass"
) -> Edit:
    default_style: types.AnnotationStyle = {
        "font_size": 20,
        "foreground": "#ffffff",
        "background": "#000000",
        "background_opacity": 160,
        "border": 1,
    }
    return (
        Edit(resource_base=resource_base)
        .clip(
            Select.marker("intro"),
            Select.marker("middle"),
            speed=Fraction(2, 3),
            hold_ms=500,
            caption="First part",
        )
        .clip(Select.marker("middle"), Select.marker("outro"), speed=(3, 2))
        .annotate("Example", scope=Scope.whole_video())
        .annotate(
            template="frame {output_frame}",
            scope=Scope.clip(1),
            position={"anchor": "bottom_right", "x": 8, "y": 8},
            style={
                "font_size": 16,
                "foreground": "#ffff00",
                "background": "#202020",
                "background_opacity": 200,
                "border": 0,
            },
            z_order=1,
        )
        .annotate(
            "Range",
            scope=Scope.range(Select.marker("intro"), Select.marker("middle")),
            position={"anchor": "center", "x": 0, "y": 0},
            style=default_style,
            z_order=2,
        )
        .annotate(
            "Start",
            scope=Scope.output_frame(0),
            position={"anchor": "top_right", "x": 16, "y": 16},
            z_order=3,
        )
        .subtitles(subtitle)
        .changes("semantic_cells", "bounding_box", hold_frames=2)
        .caption_layout("footer")
        .branding("Example")
        .delivery(codec="h264", pixel_format="yuv420p", crf=20, encoder_preset="slow")
        .cursor_mask()
        .crop(0, 0, 480, 240)
        .zoom(2, 1)
        .callout(16, 16, 200, 100, label="note")
    )


@pytest.fixture(scope="module")
def edit_workspace(
    tmp_path_factory: pytest.TempPathFactory,
) -> tuple[Termtrace, Path, Path]:
    root = tmp_path_factory.mktemp("editing").resolve()
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(root / "evidence"))
    grid = EDITING["session"]["grid"]
    with tt.record(
        ["/bin/sh", "-c", EDIT_SESSION_SCRIPT],
        rows=grid["rows"],
        cols=grid["cols"],
        visual_profile=SEMANTIC_PROFILE,
    ) as rec:
        rec.screen().wait_for_text("line 1", timeout_secs=5.0)
        for delay, label in [
            (0.3, "intro"),
            (0.6, "middle"),
            (0.1, "repeat"),
            (0.1, "repeat"),
            (0.5, "outro"),
        ]:
            time.sleep(delay)
            rec.mark(label)
        time.sleep(0.3)
        rec.stop()
    return tt, Path(rec.path), root


def plan_semantic(tt: Termtrace, session: Path, edit: Any) -> types.PresentationPlan:
    return tt.video_plan(session, edit, renderer="semantic", fps=EDITING["fps"])


def without_subtitle_paths(plan: types.PresentationPlan) -> types.PresentationPlan:
    copy = json.loads(json.dumps(plan))
    for subtitle in copy["subtitles"]:
        subtitle["source_path"] = None
    return copy  # type: ignore[no-any-return]


def edit_file(directory: Path, edit: Any, files: dict[str, str] | None = None) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    for name, contents in (files or {}).items():
        (directory / name).write_text(contents, encoding="utf-8")
    path = directory / "edit.json"
    path.write_text(json.dumps(edit), encoding="utf-8")
    return path


def test_fixture_edit_built_in_python_matches_the_rust_normalization_and_file_plan(
    edit_workspace: tuple[Termtrace, Path, Path],
) -> None:
    tt, session, root = edit_workspace
    edit = fixture_edit(SDK_FIXTURES)
    normalized = edit.normalize()
    assert normalized["edit_plan_hash"] == EDITING["edit_plan_hash"]

    copy = root / "fixture-file"
    copy.mkdir()
    shutil.copy(SDK_FIXTURES / "editing-captions.ass", copy / "editing-captions.ass")
    from_file = plan_semantic(tt, session, edit_file(copy, EDITING["edit"]))
    from_builder = plan_semantic(tt, session, edit)
    assert from_builder["edit_plan_hash"] == EDITING["edit_plan_hash"]
    assert without_subtitle_paths(from_builder) == without_subtitle_paths(from_file)

    for frame in range(from_builder["output_frame_count"]):
        found = source_for_output_frame(from_builder, frame)
        assert found is not None
        spans = output_frames_for_source_frame(from_builder, found["source_frame"])
        assert any(start <= frame < end for start, end in spans)
    assert (
        source_for_output_frame(from_builder, from_builder["output_frame_count"])
        is None
    )


def test_relative_subtitles_need_an_explicit_resource_base() -> None:
    expected = EDITING["relative_without_base"]
    with pytest.raises(TermtraceError) as caught:
        fixture_edit(None).normalize()
    assert caught.value.code == expected["code"]
    assert {key: caught.value.details[key] for key in expected["facts"]} == expected[
        "facts"
    ]

    absolute = fixture_edit(None, SDK_FIXTURES / "editing-captions.ass")
    assert absolute.normalize()["edit_plan_hash"] == EDITING["edit_plan_hash"]

    with tempfile.TemporaryDirectory() as elsewhere:
        as_file = edit_file(Path(elsewhere), EDITING["edit"])
        shutil.copy(
            SDK_FIXTURES / "editing-captions.ass",
            Path(elsewhere) / "editing-captions.ass",
        )
        assert normalize_edit(as_file)["edit_plan_hash"] == EDITING["edit_plan_hash"]


def test_selectors_keep_exact_64_bit_values_and_reject_inexact_host_types() -> None:
    wide = 2**63 + 1
    normalized = Edit().clip(Select.event(wide), Select.state(_U64_MAX)).normalize()
    clip = normalized["edit"]["clips"][0]
    assert clip["from"]["eventSeq"] == wide
    assert clip["to"]["stateId"] == _U64_MAX

    for value, error in [
        (1.0, TypeError),
        (True, TypeError),
        (-1, ValueError),
        (2**64, ValueError),
    ]:
        with pytest.raises(error):
            Select.event(value)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        Edit().clip(Select.marker("a"), Select.marker("b"), speed=0.5)  # type: ignore[arg-type]


def test_speeds_produce_the_documented_frame_counts(
    edit_workspace: tuple[Termtrace, Path, Path],
) -> None:
    tt, session, _ = edit_workspace
    speeds = EDITING["speeds"]

    def edit(speed: tuple[int, int]) -> Edit:
        return (
            Edit()
            .clip(
                Select.at_ns(speeds["from_ns"]),
                Select.at_ns(speeds["to_ns"]),
                speed=speed,
            )
            .clip(Select.at_ns(speeds["to_ns"]), Select.at_ns(speeds["tail_to_ns"]))
        )

    for case in speeds["cases"]:
        clip = plan_semantic(tt, session, edit(tuple(case["speed"])))["clips"][0]
        assert (
            clip["source_end_frame"] - clip["source_start_frame"]
            == speeds["source_frames"]
        )
        assert clip["retimed_frame_count"] == case["retimed_frame_count"], case

    for speed in speeds["invalid"]:
        with pytest.raises(TermtraceError) as caught:
            edit(tuple(speed)).normalize()
        assert caught.value.code == "invalid_speed"
        assert caught.value.details["field"] == "clips[0].speed"


def test_shared_negative_edits_fail_with_the_same_codes_and_facts(
    edit_workspace: tuple[Termtrace, Path, Path],
) -> None:
    tt, session, root = edit_workspace
    for case in EDITING["negative"]:
        path = edit_file(
            root / "negative" / case["name"], case["edit"], case.get("files")
        )
        with pytest.raises(TermtraceError) as caught:
            plan_semantic(tt, session, path)
        assert caught.value.code == case["code"], case["name"]
        facts = {key: caught.value.details[key] for key in case["facts"]}
        assert facts == case["facts"], case["name"]


async def test_semantic_preview_and_every_format_render_from_python(
    edit_workspace: tuple[Termtrace, Path, Path],
) -> None:
    tt, session, root = edit_workspace
    edit = fixture_edit(SDK_FIXTURES)
    output = root / "renders"
    output.mkdir()

    preview = await AsyncTermtrace(binary=TERMTRACE_BIN).video_preview(
        session, edit, output / "preview", renderer="semantic", fps=EDITING["fps"]
    )
    assert Path(preview["contact_sheet"]).is_file()
    assert preview["plan"]["edit_plan_hash"] == EDITING["edit_plan_hash"]

    completion = json.loads((session / "completion.json").read_text(encoding="utf-8"))
    assert json.loads((session / "session.json").read_text(encoding="utf-8"))[
        "visual_profile"
    ]
    for extension, codec in [("mp4", "h264"), ("gif", "gif"), ("webm", "vp9")]:
        out = output / f"edited.{extension}"
        render = tt.video_render(
            session, edit, out, renderer="semantic", fps=EDITING["fps"]
        )
        assert Path(render["output"]) == out
        probe = subprocess.run(
            [
                "ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "csv=p=0",
                str(out),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        assert probe.stdout.strip() == codec
        manifest = json.loads(
            (Path(render["sidecar"]) / "manifest.json").read_text(encoding="utf-8")
        )
        if extension == "mp4":
            assert render["artifact"] is None
            assert manifest["source_truth_layer"] == "semantic_rendering"
        else:
            artifact = render["artifact"]
            assert artifact is not None
            assert artifact["provenance"]["source_kind"] == "semantic_rendering"
            assert (
                artifact["source"]["session_manifest_hash"]
                == completion["session_manifest_hash"]
            )

    existing = output / "edited.mp4"
    before = existing.read_bytes()
    with pytest.raises(TermtraceError, match="already exists"):
        tt.video_render(
            session, edit, existing, renderer="semantic", fps=EDITING["fps"]
        )
    assert existing.read_bytes() == before
    with pytest.raises(TermtraceError, match="already exists"):
        tt.video_render(
            session, edit, output / "edited.gif", renderer="semantic", overwrite=True
        )


TERMINAL_PROGRAMS = Path(__file__).resolve().parents[3] / "fixtures/programs/terminal"
INPUT_REPORTER = [str(TERMINAL_PROGRAMS / "input-reporter.py")]
LINE_COMMAND = [str(TERMINAL_PROGRAMS / "line-command.sh")]


def reported(text: str, event: str) -> list[int]:
    return [
        int(ms)
        for ms in re.findall(rf"^{re.escape(event)} T=(\d+)", text, re.MULTILINE)
    ]


def wait_for_completed(session: Any) -> dict[str, Any]:
    deadline = time.monotonic() + 10
    while True:
        try:
            status = session.status()
        except TermtraceError:
            status = None
        if status is not None and status["kind"] == "completed":
            return status
        assert time.monotonic() < deadline, "session did not complete"
        time.sleep(0.05)


def launch_reporter(tt: Termtrace) -> Recording:
    rec = tt.launch(INPUT_REPORTER, {"rows": 40, "cols": 80})
    rec.screen().wait_for_text("REPORTER_READY", timeout_secs=5.0)
    return rec


def test_launch_returns_a_ready_session_and_fails_past_its_readiness_deadline(
    tmp_path: Path,
) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with tt.launch(["/bin/sh", "-c", "printf STARTED; sleep 30"]) as rec:
        assert rec.status()["kind"] == "live"
        assert "STARTED" in rec.screen().wait_for_text("STARTED", timeout_secs=5.0).text

    with pytest.raises(TermtraceError, match="readiness timed out"):
        tt.launch(["/bin/sh", "-c", "sleep 30"], {"startup_timeout_ms": 1})


def test_launch_runs_the_child_in_the_requested_working_directory(
    tmp_path: Path,
) -> None:
    workdir = tmp_path / "child-workdir"
    workdir.mkdir()
    (workdir / "marker-file.txt").write_text("marker", encoding="utf-8")
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with tt.launch(
        [
            "/bin/sh",
            "-c",
            "printf 'CWD=%s\\r\\n' \"$(pwd -P)\"; ls; printf DONE; sleep 30",
        ],
        {"cwd": str(workdir), "cols": 200},
    ) as rec:
        text = rec.screen().wait_for_text("DONE", timeout_secs=5.0).text
        assert f"CWD={workdir.resolve()}" in text
        assert "marker-file.txt" in text

    with pytest.raises(TermtraceError):
        tt.launch(["/bin/sh", "-c", "sleep 30"], {"cwd": str(tmp_path / "missing")})


def test_detached_session_keeps_running_and_is_controlled_through_its_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    state = tmp_path / "state"
    monkeypatch.setenv("TERMTRACE_STATE_DIR", str(state))
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    rec = tt.launch(LINE_COMMAND)
    try:
        rec.screen().wait_for_text("LINE_READY", timeout_secs=5.0)
        tt.registry(str(state)).register("detached-demo", rec.path)
        rec.detach()
        assert not rec.is_active()
        with pytest.raises(RuntimeError):
            rec.detach()

        by_name = tt.attach_target("name:detached-demo")
        by_path = tt.attach_target(f"path:{rec.path}")
        assert by_name.session_id == by_path.session_id == rec.session_id
        assert by_name.status()["kind"] == "live"
        assert "LINE_READY" in by_path.screen().text()

        outcome = (
            by_name.actions()
            .send_text("detached-input")
            .send_key("Enter")
            .until_screen("LINE_VALUE=detached-input", timeout_secs=5.0)
            .execute()
        )
        assert "LINE_VALUE=detached-input" in outcome.text
        assert wait_for_completed(by_path)["completion"]["exit_code"] == 0

        with pytest.raises(TermtraceError):
            tt.attach_target("name:unregistered-demo")
    finally:
        if not (Path(rec.path) / "completion.json").exists():
            tt.attach(rec.path).signal("terminate")


def test_paste_focus_and_scroll_reach_an_application_that_reports_them(
    tmp_path: Path,
) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with launch_reporter(tt) as rec:
        pasted = (
            rec.actions()
            .paste("pasted words")
            .until_screen("PASTE=", timeout_secs=5.0)
            .execute()
        )
        assert len(reported(pasted.text, "PASTE=pasted words")) == 1
        assert not reported(pasted.text, "KEY=70")

        pointer = rec.pointer()
        assert pointer.focus(False, strict=True)["kind"] == "applied"
        rec.screen().wait_for_text("FOCUS=out", timeout_secs=5.0)
        assert pointer.focus(True, strict=True)["kind"] == "applied"
        rec.screen().wait_for_text("FOCUS=in", timeout_secs=5.0)

        assert pointer.scroll(4, 2, lines=-2, strict=True)["kind"] == "applied"
        assert pointer.scroll(9, 7, lines=1, strict=True)["kind"] == "applied"
        text = rec.screen().wait_for_text("WHEEL=down", timeout_secs=5.0).text
        assert len(reported(text, "WHEEL=up@5,3")) == 2
        assert len(reported(text, "WHEEL=down@10,8")) == 1

    with launch_fixture(tt, "echo_command") as rec:
        assert rec.pointer().focus(True)["kind"] == "noop"
        assert rec.pointer().scroll(0, 0, lines=1)["kind"] == "noop"
        with pytest.raises(TermtraceError):
            rec.pointer().focus(True, strict=True)


def test_paced_text_arrives_one_character_at_a_time(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with launch_reporter(tt) as rec:
        rec.actions().send_text("abc").until_screen(
            "KEY=63", timeout_secs=5.0
        ).execute()
        text = (
            rec.actions()
            .pace(200)
            .send_text("xyz")
            .until_screen("KEY=7a", timeout_secs=5.0)
            .execute()
            .text
        )
        burst = [reported(text, f"KEY={key}")[0] for key in ("61", "62", "63")]
        paced = [reported(text, f"KEY={key}")[0] for key in ("78", "79", "7a")]
        assert max(burst) - min(burst) < 100
        assert all(
            later - earlier >= 150 for earlier, later in itertools.pairwise(paced)
        )


def test_wait_for_idle_returns_after_output_stops(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    ticker = "i=0; while [ $i -lt 8 ]; do i=$((i+1)); printf 'TICK %d\\r\\n' $i; sleep 0.2; done"
    with tt.launch(["/bin/sh", "-c", f"{ticker}; printf SETTLED; sleep 30"]) as rec:
        started = time.monotonic()
        state = rec.screen().wait_for_idle(timeout_secs=15.0)
        assert time.monotonic() - started >= 1.4
        assert "TICK 8" in state.text
        assert "SETTLED" in state.text

    with (
        tt.launch(
            ["/bin/sh", "-c", "echo BUSY; while :; do date +%s%N; sleep 0.1; done"]
        ) as rec,
        pytest.raises(TermtraceError, match="stable interval"),
    ):
        # A blank screen before the first output would itself be idle.
        rec.screen().wait_for_text("BUSY", timeout_secs=5.0)
        rec.screen().wait_for_idle(timeout_secs=1.5)


def test_observe_selects_a_hidden_synchronized_state_only_when_asked(
    tmp_path: Path,
) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    rec = tt.launch(
        ["/bin/sh", "-c", "printf '\\033[?2026hHIDDEN-PART\\033[?2026l VISIBLE-PART'"]
    )
    with rec:
        rec.output().wait_for(b"VISIBLE-PART", 5)
    screen = tt.attach(rec.path).screen()
    hidden = [
        entry["item"]
        for entry in rec.analysis().timeline(limit=1000, include_hidden=True)["entries"]
        if entry["item"]["kind"] == "semantic_state" and not entry["item"]["presented"]
    ][-1]
    point = {"selector": "state_id", "state_id": hidden["state_id"]}

    observed = screen.observe(point, include_hidden=True)
    assert observed["presented"] is False
    assert observed["state_id"] == hidden["state_id"]
    assert "HIDDEN-PART" in observed["frame"]["viewport_text"]
    assert "VISIBLE-PART" not in observed["frame"]["viewport_text"]
    with pytest.raises(TermtraceError):
        screen.observe(point)
    latest = screen.observe()
    assert latest["presented"] is True
    assert "VISIBLE-PART" in latest["frame"]["viewport_text"]


def test_cache_status_reports_the_checkpoint_a_completed_observation_writes(
    tmp_path: Path,
) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    rec = tt.launch(["/bin/sh", "-c", "printf CACHED-SCREEN"])
    with rec:
        rec.output().wait_for(b"CACHED-SCREEN", 5)
    session = tt.attach(rec.path)
    before = session.analysis().cache_status()
    assert (before["entries"], before["compressed_bytes"]) == (0, 0)

    assert session.screen().observe()["resolution"] == "canonical_replay"
    after = session.analysis().cache_status()
    assert after["entries"] == 1
    assert after["compressed_bytes"] > 0
    assert os.fsdecode(after["directory"]).startswith(rec.path)
    assert session.screen().observe()["resolution"] == "validated_checkpoint"

    session.analysis().cache_prune()
    assert session.analysis().cache_status()["entries"] == 0


def test_discard_completed_removes_only_a_completed_session(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with launch_fixture(tt, "echo_command") as rec:
        with pytest.raises(TermtraceError):
            rec.discard_completed()
        assert rec.status()["kind"] == "live"
        assert Path(rec.path).is_dir()

    assert rec.status()["kind"] == "completed"
    rec.discard_completed()
    assert not Path(rec.path).exists()
    with pytest.raises(TermtraceError):
        tt.attach(rec.path)
    with pytest.raises(TermtraceError):
        rec.discard_completed()


@pytest.mark.asyncio
async def test_async_paste_pacing_focus_and_scroll_reach_an_application_that_reports_them(
    tmp_path: Path,
) -> None:
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    async with await tt.launch(INPUT_REPORTER, {"rows": 40, "cols": 80}) as rec:
        await rec.screen().wait_for_text("REPORTER_READY", timeout_secs=5.0)
        pasted = (
            await rec.actions()
            .paste("async words")
            .until_screen("PASTE=", timeout_secs=5.0)
            .run()
        )
        assert pasted.frame is not None
        assert len(reported(pasted.frame.text, "PASTE=async words")) == 1
        assert not reported(pasted.frame.text, "KEY=61")

        paced = (
            await rec.actions()
            .pace(200)
            .send_text("xyz")
            .until_screen("KEY=7a", timeout_secs=5.0)
            .run()
        )
        assert paced.frame is not None
        arrivals = [
            reported(paced.frame.text, f"KEY={key}")[0] for key in ("78", "79", "7a")
        ]
        assert all(
            later - earlier >= 150 for earlier, later in itertools.pairwise(arrivals)
        )

        pointer = rec.pointer()
        assert (await pointer.focus(False, strict=True))["kind"] == "applied"
        await rec.screen().wait_for_text("FOCUS=out", timeout_secs=5.0)
        assert (await pointer.scroll(2, 3, lines=1, strict=True))["kind"] == "applied"
        text = (await rec.screen().wait_for_text("WHEEL=down", timeout_secs=5.0)).text
        assert len(reported(text, "WHEEL=down@3,4")) == 1


@pytest.mark.asyncio
async def test_async_launch_target_idle_observe_cache_and_discard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    state = tmp_path / "state"
    monkeypatch.setenv("TERMTRACE_STATE_DIR", str(state))
    workdir = tmp_path / "async-workdir"
    workdir.mkdir()
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with pytest.raises(TermtraceError, match="readiness timed out"):
        await tt.launch(["/bin/sh", "-c", "sleep 30"], {"startup_timeout_ms": 1})

    ticker = "i=0; while [ $i -lt 6 ]; do i=$((i+1)); printf 'TICK %d\\r\\n' $i; sleep 0.1; done"
    script = (
        "printf 'CWD=%s\\r\\n' \"$(pwd -P)\"; "
        f"{ticker}; printf '\\033[?2026hHIDDEN-PART\\033[?2026l VISIBLE-PART'; read -r _"
    )
    rec = await tt.launch(["/bin/sh", "-c", script], {"cwd": str(workdir), "cols": 120})
    try:
        assert (await rec.status())["kind"] == "live"
        await rec.screen().wait_for_text("TICK 1", timeout_secs=5.0)
        started = time.monotonic()
        idle = await rec.screen().wait_for_idle(timeout_secs=15.0)
        assert time.monotonic() - started >= 0.4
        assert "TICK 6" in idle.text
        assert f"CWD={workdir.resolve()}" in idle.text
        assert "VISIBLE-PART" in idle.text

        await tt.registry(str(state)).register("async-target", rec.path)
        session = await tt.attach_target("name:async-target")
        assert session.session_id == rec.session_id
        await session.actions().send_key("Enter").run()
        assert (await rec.wait_for_completion(timeout_ms=10_000))["exit_code"] == 0
        with pytest.raises(TermtraceError):
            await tt.attach_target("name:missing-target")
    finally:
        if await rec.is_active():
            await rec.force_close()

    completed = await tt.attach(rec.path)
    hidden = [
        entry["item"]
        for entry in (
            await completed.analysis().timeline(limit=1000, include_hidden=True)
        )["entries"]
        if entry["item"]["kind"] == "semantic_state" and not entry["item"]["presented"]
    ][-1]
    point: types.StatePoint = {"selector": "state_id", "state_id": hidden["state_id"]}
    assert (await completed.analysis().cache_status())["entries"] == 0
    observed = await completed.screen().observe(point, include_hidden=True)
    assert observed["presented"] is False
    assert "HIDDEN-PART" in observed["frame"]["viewport_text"]
    assert "VISIBLE-PART" not in observed["frame"]["viewport_text"]
    with pytest.raises(TermtraceError):
        await completed.screen().observe(point)
    assert (await completed.analysis().cache_status())["entries"] >= 1

    await rec.discard_completed()
    assert not Path(rec.path).exists()
    with pytest.raises(TermtraceError):
        await tt.attach(rec.path)
