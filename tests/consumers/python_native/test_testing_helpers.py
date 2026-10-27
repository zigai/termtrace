"""Real immutable assertion/snapshot/helper parity; no unavailable passing skips."""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest
from termtrace import LocatorSpec, QueryError, ScreenFrame, Termtrace
from termtrace import testing as t

REPO = Path(__file__).resolve().parents[3]
BINARY = os.environ.get("TERMTRACE_BIN", str(REPO / "target/debug/termtrace"))
CORPUS = json.loads((REPO / "fixtures/conformance/testing/assertions.json").read_text())
FRAME = (REPO / "fixtures/conformance/locators/parity-frame.json").read_text()
COMMAND = [
    "/bin/sh",
    "-c",
    "trap 'exit 0' TERM; printf READY; while IFS= read -r line; do printf 'INPUT:<%s>' \"$line\"; done",
]


def corpus_check() -> None:
    f = ScreenFrame.from_json(FRAME)
    found = f.query(LocatorSpec.text("Save"))
    none = f.query(LocatorSpec.text("ABSENT"))
    selected = found.first()
    for profile, expected in CORPUS["normalized"].items():
        assert (
            t.normalized_snapshot(
                f, profile=profile, include_cursor=True, include_modes=True
            )
            == expected
        )
    assert t.masked_snapshot(f, CORPUS["mask_options"]) == CORPUS["masked"]
    t.assert_screen_text(f, "Save")
    t.assert_exists(found)
    t.assert_not_exists(none)
    t.assert_count(found, 2)
    t.assert_match_text(selected, "Save")
    t.assert_match_style(selected, {})
    t.assert_cursor(f, f.cursor)
    t.assert_no_presented_transient(f)
    bad = json.loads(FRAME)
    bad["presented"] = False
    calls = {
        "screen_text": lambda: t.assert_screen_text(f, "ABSENT"),
        "exists": lambda: t.assert_exists(none),
        "not_exists": lambda: t.assert_not_exists(found),
        "count": lambda: t.assert_count(found, 1),
        "match_text": lambda: t.assert_match_text(selected, "OTHER"),
        "match_style": lambda: t.assert_match_style(selected, {"bold": True}),
        "cursor": lambda: t.assert_cursor(f, {**f.cursor, "row": 0}),
        "no_wrapped_rows": lambda: t.assert_no_wrapped_rows(f),
        "neutral_region": lambda: t.assert_neutral_region(
            f, {"top": 0, "left": 0, "right": 1, "bottom": 1}
        ),
        "no_presented_transient": lambda: t.assert_no_presented_transient(
            ScreenFrame.from_json(json.dumps(bad))
        ),
    }
    for name, call in calls.items():
        with pytest.raises(t.ExpectationMismatchError) as caught:
            call()
        assert isinstance(caught.value, AssertionError)
        assert {
            k: getattr(caught.value, k) for k in CORPUS["mismatches"][name]
        } == CORPUS["mismatches"][name]
        t.assert_not(call)
    for operation in [
        lambda: f.query(LocatorSpec.regex("[")),
        lambda: found.single(),
        lambda: none.first(),
    ]:
        with pytest.raises(QueryError):
            t.assert_not(operation)
    for detail in ["text", "none"]:
        value = json.loads(FRAME)
        value["data"] = (
            {"detail": detail}
            if detail == "none"
            else {
                "detail": "text",
                "rows": [
                    {
                        k: r[k]
                        for k in ["row", "text", "soft_wrapped", "wrap_continuation"]
                    }
                    for r in value["data"]["rows"]
                ],
            }
        )
        frame = ScreenFrame.from_json(json.dumps(value))
        if detail == "text":
            with pytest.raises(QueryError):
                t.assert_not(
                    lambda frame=frame: t.assert_match_style(
                        frame.query(LocatorSpec.text("Save")).first(), {}
                    )
                )
        else:
            with pytest.raises(QueryError):
                t.assert_not(lambda frame=frame: t.assert_screen_text(frame, "Save"))
        with pytest.raises(ValueError):
            t.masked_snapshot(frame, CORPUS["mask_options"])


@pytest.mark.parametrize("offload", [False, True], ids=["sync", "async"])
def test_shared_testing_corpus(offload: bool) -> None:
    if offload:
        asyncio.run(asyncio.to_thread(corpus_check))
    else:
        corpus_check()


def test_file_snapshots_and_nonnegatable_filesystem_errors(tmp_path: Path) -> None:
    frame = ScreenFrame.from_json(FRAME)
    path = tmp_path / "baseline.json"
    with pytest.raises(t.ExpectationMismatchError):
        t.assert_snapshot_file(frame, path)
    t.assert_snapshot_file(frame, path, update=True)
    original = path.read_bytes()
    t.assert_snapshot_file(frame, path)
    assert original == (t.normalized_snapshot(frame) + "\n").encode()
    link = tmp_path / "link"
    link.symlink_to(path)
    for update in [False, True]:
        with pytest.raises(Exception) as error:
            t.assert_not(
                lambda update=update: t.assert_snapshot_file(frame, link, update=update)
            )
        assert not isinstance(error.value, t.ExpectationMismatchError)
    bad = {"mask_regions": [{"top": 1, "left": 0, "right": 1, "bottom": 2}]}
    with pytest.raises(QueryError):
        t.assert_masked_snapshot_file(frame, path, bad, update=True)
    assert path.read_bytes() == original
    masked = tmp_path / "masked.json"
    t.assert_masked_snapshot_file(frame, masked, CORPUS["mask_options"], update=True)
    t.assert_masked_snapshot_file(frame, masked, CORPUS["mask_options"])


def evidence_parent(tmp_path: Path) -> Path:
    configured = os.environ.get("TASK04_EVIDENCE")
    if not configured:
        return tmp_path
    parent = Path(configured)
    parent.mkdir(parents=True, exist_ok=True)
    # Every verification attempt is fresh. Never count/reuse/delete retained evidence from a
    # previous invocation just because pytest recycled a human-readable test-directory name.
    return Path(tempfile.mkdtemp(prefix=tmp_path.name + "-", dir=parent))


@pytest.mark.parametrize("policy", ["on_failure", "always", "manual"])
def test_real_retention_action_and_exactly_once(tmp_path: Path, policy: str) -> None:
    parent = evidence_parent(tmp_path)
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(parent), evidence_policy=policy
    )
    with context:
        rec = context.record(COMMAND)
        rec.screen().wait_for_text("READY", 5)
        result = rec.actions().send_text("go\n").until_screen("INPUT:<go>", 5).run()
        t.assert_screen_text(result.frame, "INPUT:<go>")
        report = context.finalize()
        assert context.finalize("error") is report
        assert not rec.is_active()
    assert report["secondary_failures"] == []
    assert report["retained"] == (policy != "on_failure")
    assert Path(context.evidence_root).exists() == report["retained"]
    if report["retained"]:
        assert Path(rec.path, "completion.json").is_file()
        verified = Termtrace(binary=BINARY).verify_session(rec.path, "content")
        assert verified["valid"]
        (parent / "proof.json").write_text(
            json.dumps({"report": report, "verification": verified})
        )
    with pytest.raises(RuntimeError):
        context.record(COMMAND)


@pytest.mark.parametrize(
    "failure", [RuntimeError("PRIMARY"), KeyboardInterrupt("PRIMARY")]
)
def test_primary_baseexception_and_real_surviving_evidence(
    tmp_path: Path, failure: BaseException
) -> None:
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    with pytest.raises(type(failure)) as raised, context:
        rec = context.record(COMMAND)
        rec.screen().wait_for_text("READY", 5)
        raise failure
    assert raised.value is failure
    assert context.report["retained"]
    assert context.report["secondary_failures"] == []
    for path in context.report["verified_artifacts"]:
        assert Path(path).exists()
    assert Termtrace(binary=BINARY).verify_session(rec.path, "content")["valid"]


def test_cleanup_failure_does_not_prevent_sibling_finalization(tmp_path: Path) -> None:
    context = t.terminal_test(
        binary=BINARY,
        evidence_root=str(evidence_parent(tmp_path)),
        finalization_timeout_secs=0.05,
    )
    with pytest.raises(t.FinalizationError) as failure, context:
        first = context.record(COMMAND)
        second = context.record(COMMAND)
        first.screen().wait_for_text("READY", 5)
        second.screen().wait_for_text("READY", 5)
        first.force_close()  # no validated completion: a genuine cleanup/verification failure
    assert failure.value.report["secondary_failures"]
    assert Path(second.path, "completion.json").is_file()
    assert not second.is_active()
    assert context.report["retained"]


@pytest.mark.asyncio
async def test_async_cancellation_owns_late_launch_and_is_responsive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    context = t.async_terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    original = context._context.record
    import threading

    started = threading.Event()

    def delayed(*args, **kwargs):
        started.set()
        time.sleep(0.15)
        return original(*args, **kwargs)

    monkeypatch.setattr(context._context, "record", delayed)
    heartbeat = 0

    async def body():
        async with context:
            launch = asyncio.create_task(context.record(COMMAND))
            while not started.is_set():
                await asyncio.sleep(0.001)
            launch.cancel()
            with pytest.raises(asyncio.CancelledError):
                await launch
            raise asyncio.CancelledError("PRIMARY_CANCEL")

    task = asyncio.create_task(body())
    while not task.done():
        heartbeat += 1
        await asyncio.sleep(0.01)
        if heartbeat > 2:
            task.cancel("REPEATED_CANCEL")
    with pytest.raises(asyncio.CancelledError) as failure:
        await task
    assert failure.value.args == ("PRIMARY_CANCEL",)
    assert heartbeat > 5
    assert context.report["retained"]
    assert len(context._context._recordings) == 1
    rec = context._context._recordings[0]
    assert not rec.is_active()
    assert Path(rec.path, "completion.json").is_file()


@pytest.mark.parametrize(
    "outcome,exitcode",
    [
        ("pass", 0),
        ("call", 1),
        ("setup", 1),
        ("teardown", 1),
        ("skip", 0),
        ("xfail", 0),
        ("cleanup", 1),
    ],
)
def test_pytest_outcomes_are_truthful(
    tmp_path: Path, outcome: str, exitcode: int
) -> None:
    # This subprocess is already inside the parent's verification lock; it acquires no nested lock.
    test = tmp_path / "test_outcomes.py"
    test.write_text(f"""import pytest, pathlib
@pytest.fixture
def owned(termtrace_session):
    rec = termtrace_session({COMMAND!r})
    rec.screen().wait_for_text("READY", 5)
    pathlib.Path({str(tmp_path / "path")!r}).write_text(rec.path)
    if {outcome!r} == "setup": raise RuntimeError("SETUP_SENTINEL")
    yield rec
    if {outcome!r} == "teardown": raise RuntimeError("TEARDOWN_SENTINEL")
def test_body(owned):
    if {outcome!r} == "call": raise AssertionError("CALL_SENTINEL")
    if {outcome!r} == "skip": pytest.skip("SKIP_SENTINEL")
    if {outcome!r} == "xfail": pytest.xfail("XFAIL_SENTINEL")
    if {outcome!r} == "cleanup": owned.force_close()
""")
    run = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", str(test)],
        env={**os.environ, "TERMTRACE_BIN": BINARY},
        check=False,
        capture_output=True,
        text=True,
        timeout=35,
    )
    assert run.returncode == exitcode, run.stdout + run.stderr
    path = Path((tmp_path / "path").read_text())
    assert path.exists() == (outcome != "pass"), run.stdout + run.stderr
    if outcome != "cleanup" and outcome != "pass":
        assert (path / "completion.json").is_file()
    if outcome == "cleanup":
        assert "Termtrace teardown failed" in run.stdout


@pytest.mark.asyncio
async def test_async_real_action_and_success_cleanup(tmp_path: Path) -> None:
    context = t.async_terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    async with context:
        rec = await context.record(COMMAND)
        await rec.screen().wait_for_text("READY", 5)
        result = (
            await rec.actions()
            .send_text("async\n")
            .until_screen("INPUT:<async>", 5)
            .run()
        )
        t.assert_screen_text(result.frame, "INPUT:<async>")
        t.assert_cursor(result.frame, result.frame.cursor)
    assert context.report["secondary_failures"] == []
    assert not Path(context.evidence_root).exists()
    assert not await rec.is_active()


@pytest.mark.asyncio
async def test_cancellation_during_passing_cleanup_retains_and_is_shielded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import threading

    context = t.async_terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    original = t._native.finalize_recordings
    started = threading.Event()

    def slow_finalize(*args):
        started.set()
        time.sleep(0.15)
        return original(*args)

    monkeypatch.setattr(t._native, "finalize_recordings", slow_finalize)

    async def body():
        async with context:
            rec = await context.record(COMMAND)
            await rec.screen().wait_for_text("READY", 5)

    task = asyncio.create_task(body())
    while not started.is_set():
        await asyncio.sleep(0.005)
    task.cancel("CLEANUP_CANCEL")
    while not task.done():
        await asyncio.sleep(0.01)
        task.cancel("REPEATED_CANCEL")
    with pytest.raises(asyncio.CancelledError) as failure:
        await task
    assert failure.value.args == ("CLEANUP_CANCEL",)
    assert context.report["retained"]
    assert any(
        f["phase"] == "cancellation" for f in context.report["secondary_failures"]
    )
    assert not context._context._recordings[0].is_active()


def test_body_and_cleanup_failures_keep_original_baseexception(tmp_path: Path) -> None:
    failure = KeyboardInterrupt("PRIMARY_WITH_CLEANUP")
    context = t.terminal_test(
        binary=BINARY,
        evidence_root=str(evidence_parent(tmp_path)),
        finalization_timeout_secs=0.05,
    )
    with pytest.raises(KeyboardInterrupt) as caught, context:
        rec = context.record(COMMAND)
        rec.screen().wait_for_text("READY", 5)
        rec.force_close()
        raise failure
    assert caught.value is failure
    assert context.report["secondary_failures"]
    assert context.report["retained"]


@pytest.mark.parametrize("asynchronous", [False, True])
def test_legacy_recording_context_preserves_primary_and_layered_finalization(
    tmp_path: Path, asynchronous: bool
) -> None:
    failure = KeyboardInterrupt("LEGACY_PRIMARY")
    if not asynchronous:
        outer = t.terminal_test(
            binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
        )
        with (
            pytest.raises(KeyboardInterrupt) as caught,
            outer,
            outer.record(COMMAND) as rec,
        ):
            rec.screen().wait_for_text("READY", 5)
            raise failure
        assert caught.value is failure
        assert outer.report["secondary_failures"] == []
        assert len(list(Path(outer.evidence_root).glob(".test-artifacts-*"))) == 1
    else:

        async def run():
            outer = t.async_terminal_test(
                binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
            )
            with pytest.raises(KeyboardInterrupt) as caught:
                async with outer, await outer.record(COMMAND) as rec:
                    await rec.screen().wait_for_text("READY", 5)
                    raise failure
            assert caught.value is failure
            assert outer.report["secondary_failures"] == []
            assert len(list(Path(outer.evidence_root).glob(".test-artifacts-*"))) == 1

        asyncio.run(run())


@pytest.mark.asyncio
async def test_pending_mutation_join_is_bounded_and_does_not_claim_rollback(
    tmp_path: Path,
) -> None:
    context = t.async_terminal_test(
        binary=BINARY,
        evidence_root=str(evidence_parent(tmp_path)),
        finalization_timeout_secs=0.05,
    )
    started = time.monotonic()
    with pytest.raises(asyncio.CancelledError):
        async with context:
            rec = await context.record(COMMAND)
            await rec.screen().wait_for_text("READY", 5)
            mutation = asyncio.create_task(
                rec.actions()
                .send_text("applied\n")
                .until_screen("NEVER_PRESENT", 30)
                .run()
            )
            await asyncio.sleep(0.1)
            mutation.cancel()
            with pytest.raises(asyncio.CancelledError):
                await mutation
            raise asyncio.CancelledError("BODY_CANCEL")
    assert time.monotonic() - started < 10
    assert context.report["retained"]
    assert any(
        f["phase"] == "pending_mutation" for f in context.report["secondary_failures"]
    )
    assert not await rec.is_active()
    # A forced actor close releases its blocked input-result worker; the request is not undone.
    await asyncio.wait_for(rec.wait_pending(), 2)
    assert rec._pending == set()


def test_unattachable_baseexception_is_not_replaced(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    class Frozen(BaseException):
        def __setattr__(self, key, value):
            raise TypeError("frozen")

    failure = Frozen("FROZEN_PRIMARY")
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    with pytest.raises(Frozen) as caught, context:
        rec = context.record(COMMAND)
        rec.screen().wait_for_text("READY", 5)
        raise failure
    assert caught.value is failure
    assert "Termtrace finalization report" in caplog.text
    assert context.report["retained"]


def test_native_finalization_transport_is_exactly_once(tmp_path: Path) -> None:
    root = evidence_parent(tmp_path)
    rec = Termtrace(binary=BINARY, evidence_root=str(root)).record(COMMAND)
    rec.screen().wait_for_text("READY", 5)
    first = t._native.finalize_recordings([rec], str(root), "passed", "manual", 1)
    again = t._native.finalize_recordings([rec], str(root), "error", "manual", 1)
    assert again == first
    assert len(list(root.glob(".test-artifacts-*"))) == 1
    with pytest.raises(t.ExpectationMismatchError) as caught:
        t.assert_not(lambda: t.assert_screen_text(ScreenFrame.from_json(FRAME), "Save"))
    assert caught.value.kind == "expectation_mismatch"
    assert caught.value.assertion == "not"
    assert caught.value.frame is None


@pytest.mark.parametrize("asynchronous", [False, True])
def test_partial_launch_failure_retains_and_finalizes_prior_sessions(
    tmp_path: Path, asynchronous: bool
) -> None:
    if not asynchronous:
        context = t.terminal_test(
            binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
        )
        original = None
        with pytest.raises(Exception) as caught, context:
            rec = context.record(COMMAND)
            rec.screen().wait_for_text("READY", 5)
            try:
                context.record(COMMAND, cwd=str(tmp_path / "missing-cwd"))
            except Exception as error:
                original = error
                raise
        assert caught.value is original
        assert context.report["retained"]
        assert Path(rec.path, "completion.json").is_file()
    else:

        async def run():
            context = t.async_terminal_test(
                binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
            )
            original = None
            with pytest.raises(Exception) as caught:
                async with context:
                    rec = await context.record(COMMAND)
                    await rec.screen().wait_for_text("READY", 5)
                    try:
                        await context.record(COMMAND, cwd=str(tmp_path / "missing-cwd"))
                    except Exception as error:
                        original = error
                        raise
            assert caught.value is original
            assert context.report["retained"]
            assert any(
                f["phase"] == "session_launch"
                for f in context.report["secondary_failures"]
            )
            assert Path(rec.path, "completion.json").is_file()

        asyncio.run(run())


def test_partial_deletion_reports_only_surviving_paths(tmp_path: Path) -> None:
    assert os.geteuid() != 0, (
        "permission failure conformance requires an unprivileged verifier"
    )
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    locked = Path(context.evidence_root, "locked")
    try:
        with pytest.raises(t.FinalizationError), context:
            rec = context.record(COMMAND)
            rec.screen().wait_for_text("READY", 5)
            locked.mkdir()
            (locked / "entry").write_text("survives")
            locked.chmod(0)
        report = context.report
        assert not Path(
            rec.path
        ).exists()  # canonical deletion succeeded before root deletion failed
        assert report["retained"] == Path(context.evidence_root).exists()
        assert report["evidence_directory"] == context.evidence_root
        assert all(Path(path).exists() for path in report["verified_artifacts"])
        assert any(
            f["phase"] == "evidence_discard" for f in report["secondary_failures"]
        )
    finally:
        if locked.exists():
            locked.chmod(0o700)


def test_artifact_preparation_failure_cannot_leak_owned_sessions(
    tmp_path: Path,
) -> None:
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    with pytest.raises(t.FinalizationError), context:
        first = context.record(COMMAND)
        second = context.record(COMMAND)
        first.screen().wait_for_text("READY", 5)
        second.screen().wait_for_text("READY", 5)
        Path(context.evidence_root, f".test-artifacts-{first.session_id}").write_text(
            "collision"
        )
    assert any(
        f["phase"] == "artifact_directory" for f in context.report["secondary_failures"]
    )
    for rec in [first, second]:
        assert not rec.is_active()
        assert Path(rec.path, "completion.json").is_file()
    assert all(Path(path).exists() for path in context.report["verified_artifacts"])


def test_invalid_root_does_not_transfer_recording_ownership(tmp_path: Path) -> None:
    rec = Termtrace(binary=BINARY, evidence_root=str(tmp_path)).record(COMMAND)
    rec.screen().wait_for_text("READY", 5)
    with pytest.raises(ValueError):
        t._native.finalize_recordings(
            [rec], str(tmp_path / "wrong"), "passed", "manual", 1
        )
    assert rec.is_active()
    result = t._native.finalize_recordings([rec], str(tmp_path), "passed", "manual", 1)
    assert result["secondary_failures"] == []
    assert not rec.is_active()


def test_direct_client_recordings_are_not_owned_or_discarded(tmp_path: Path) -> None:
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )
    foreign = None
    try:
        with context:
            owned = context.record(COMMAND)
            foreign = context.termtrace.record(COMMAND)
            owned.screen().wait_for_text("READY", 5)
            foreign.screen().wait_for_text("READY", 5)
        assert not owned.is_active()
        assert foreign.is_active()
        assert Path(foreign.path, "session.json").is_file()
        assert context.report["retained"]
        assert foreign.path not in context.report["verified_artifacts"]
    finally:
        if foreign is not None:
            foreign.stop()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "operation,stalled", [("stop", True), ("completion", True), ("completion", False)]
)
async def test_cancelled_lifecycle_wait_keeps_force_authority_and_reaps_siblings(
    tmp_path: Path, operation: str, stalled: bool
) -> None:
    import signal

    parent = evidence_parent(tmp_path)
    context = t.async_terminal_test(
        binary=BINARY, evidence_root=str(parent), finalization_timeout_secs=0.25
    )
    command = [
        "/bin/sh",
        "-c",
        'printf "%s" "$PPID" > "$1"; printf "%s" $$ > "$1.child"; printf READY; while IFS= read -r line; do :; done',
        "owned-lifecycle",
    ]
    primary = None
    rec = sibling = None
    heartbeat = 0

    async def body():
        nonlocal primary, rec, sibling
        async with context:
            rec = await context.record(
                [*command, str(parent / "actor")], rows=3, cols=20
            )
            sibling = await context.record(
                [*command, str(parent / "sibling")], rows=3, cols=20
            )
            await rec.screen().wait_for_text("READY", 5)
            await sibling.screen().wait_for_text("READY", 5)
            if stalled:
                actor = int((parent / "actor").read_text())
                assert (
                    os.fsencode(rec.path) in Path(f"/proc/{actor}/cmdline").read_bytes()
                )
                os.kill(actor, signal.SIGSTOP)
            worker = asyncio.create_task(
                rec.stop() if operation == "stop" else rec.wait_for_completion(30000)
            )
            for _ in range(1000):
                if not await rec.is_active():
                    break
                await asyncio.sleep(0.001)
            else:
                raise AssertionError("lifecycle worker did not begin")
            worker.cancel("ORIGINAL_LIFECYCLE_CANCEL")
            try:
                await worker
            except asyncio.CancelledError as error:
                primary = error
                raise

    started = time.monotonic()
    task = asyncio.create_task(body())
    try:
        while not task.done():
            heartbeat += 1
            await asyncio.sleep(0.01)
            if primary is not None:
                task.cancel("REPEATED_TEARDOWN_CANCEL")
        with pytest.raises(asyncio.CancelledError) as caught:
            await task
        assert caught.value is primary
        assert caught.value.args == ("ORIGINAL_LIFECYCLE_CANCEL",)
        elapsed = time.monotonic() - started
        assert elapsed < 8, (elapsed, context.report)
        assert heartbeat > 3
        assert context.report["retained"]
        assert not await rec.is_active() and not await sibling.is_active()
        assert Path(sibling.path, "completion.json").is_file()
        for name in ["actor", "actor.child", "sibling", "sibling.child"]:
            pid = int((parent / name).read_text())
            assert not Path(f"/proc/{pid}").exists(), (name, pid, context.report)
        await asyncio.wait_for(rec.wait_pending(), 1)
        assert not rec._pending
        (parent / "lifecycle-proof.json").write_text(
            json.dumps(
                {
                    "operation": operation,
                    "stalled": stalled,
                    "seconds": elapsed,
                    "report": context.report,
                    "pids_absent": True,
                }
            )
        )
    finally:
        for recording in [rec, sibling]:
            if recording is not None:
                await asyncio.to_thread(recording._sync.force_close)


@pytest.mark.parametrize(
    "command,fatal",
    [(["/termtrace-task04-no-such-child"], True), (["/bin/sh", "-c", "exit 7"], False)],
)
def test_fatal_recorder_completion_is_not_ordinary_app_exit(
    tmp_path: Path, command: list[str], fatal: bool
) -> None:
    context = t.terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )

    def body():
        with context:
            rec = context.record(command, rows=3, cols=20)
            completion = rec.wait_for_completion(5000)
            if fatal:
                assert completion["failure"]["code"] == "spawn_failed"
            else:
                assert completion["exit_code"] == 7 and completion["failure"] is None

    if fatal:
        with pytest.raises(t.FinalizationError):
            body()
        assert context.report["retained"]
        assert any(
            f["phase"] == "session_failure" and "spawn_failed" in f["message"]
            for f in context.report["secondary_failures"]
        )
    else:
        body()
        assert context.report["secondary_failures"] == []
        assert not context.report["retained"]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "command,fatal",
    [(["/termtrace-task04-no-such-child"], True), (["/bin/sh", "-c", "exit 7"], False)],
)
async def test_async_fatal_recorder_completion_is_not_ordinary_app_exit(
    tmp_path: Path, command: list[str], fatal: bool
) -> None:
    context = t.async_terminal_test(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    )

    async def body():
        async with context:
            rec = await context.record(command, rows=3, cols=20)
            completion = await rec.wait_for_completion(5000)
            if fatal:
                assert completion["failure"]["code"] == "spawn_failed"
            else:
                assert completion["exit_code"] == 7 and completion["failure"] is None

    if fatal:
        with pytest.raises(t.FinalizationError):
            await body()
        assert context.report["retained"]
        assert any(
            f["phase"] == "session_failure" and "spawn_failed" in f["message"]
            for f in context.report["secondary_failures"]
        )
    else:
        await body()
        assert context.report["secondary_failures"] == []
        assert not context.report["retained"]


def test_acknowledged_stop_wait_zero_keeps_applied_and_restores_handle(
    tmp_path: Path,
) -> None:
    import uuid

    from termtrace import TermtraceError

    rec = Termtrace(binary=BINARY, evidence_root=str(evidence_parent(tmp_path))).record(
        [
            "/bin/sh",
            "-c",
            "trap 'sleep .3; exit 0' TERM; printf READY; while :; do sleep .05; done",
        ]
    )
    try:
        rec.screen().wait_for_text("READY", 5)
        with pytest.raises(TermtraceError) as query:
            rec.wait_for_completion(0)
        assert query.value.kind == "engine"
        assert query.value.effect == {"kind": "not_applied"}
        assert query.value.retry == "new_request"
        request_id = str(uuid.uuid4())
        with pytest.raises(TermtraceError) as stopped:
            rec.request_stop(request_id=request_id, wait_ms=0)
        error = stopped.value
        assert error.kind == error.code == "engine"
        assert error.effect == {"kind": "applied"}
        assert error.retry == "do_not_retry"
        assert error.request_id == request_id
        assert error.details is None
        assert rec.is_active(), (
            "failed completion observation must restore the original handle"
        )
        completion = rec.wait_for_completion(5000)
        assert completion["failure"] is None
        assert not rec.is_active()
        assert Path(rec.path, "completion.json").is_file()
    finally:
        if rec.is_active():
            rec.force_close()


@pytest.mark.asyncio
async def test_async_acknowledged_stop_wait_zero_keeps_applied(tmp_path: Path) -> None:
    import uuid

    from termtrace import TermtraceError
    from termtrace.aio import AsyncTermtrace

    rec = await AsyncTermtrace(
        binary=BINARY, evidence_root=str(evidence_parent(tmp_path))
    ).record(
        [
            "/bin/sh",
            "-c",
            "trap 'sleep .3; exit 0' TERM; printf READY; while :; do sleep .05; done",
        ]
    )
    try:
        await rec.screen().wait_for_text("READY", 5)
        with pytest.raises(TermtraceError) as query:
            await rec.wait_for_completion(0)
        assert query.value.effect == {"kind": "not_applied"}
        request_id = str(uuid.uuid4())
        with pytest.raises(TermtraceError) as stopped:
            await rec.request_stop(request_id=request_id, wait_ms=0)
        assert stopped.value.kind == stopped.value.code == "engine"
        assert stopped.value.effect == {"kind": "applied"}
        assert stopped.value.retry == "do_not_retry"
        assert stopped.value.request_id == request_id
        assert stopped.value.details is None
        assert await rec.is_active()
        completion = await rec.wait_for_completion(5000)
        assert completion["failure"] is None
        assert not await rec.is_active()
    finally:
        if await rec.is_active():
            await asyncio.to_thread(rec._sync.force_close)
