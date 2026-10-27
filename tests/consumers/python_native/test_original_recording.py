"""Original recordings against the real Ghostty, Xvfb and FFmpeg toolchain."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import subprocess
import time
from pathlib import Path

import pytest
from termtrace import (
    CancelToken,
    OriginalCaptureFailedError,
    OriginalRecording,
    Termtrace,
    TermtraceError,
)
from termtrace import types
from termtrace.aio import AsyncTermtrace

TERMTRACE_BIN = os.path.abspath(
    os.environ.get("TERMTRACE_BIN", "target/debug/termtrace")
)
OPTIONS: types.OriginalLaunchOptionsInput = {
    "grid": {"cols": 80, "rows": 24},
    "canvas": {"width": 1280, "height": 720},
    "startup_timeout_ms": 60_000,
}
LOOP = "trap 'echo winch $(stty size)' WINCH; echo ready; while :; do sleep 0.05; done"


def wait_for_output(original: OriginalRecording, text: bytes) -> None:
    session = original.session()
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if session.wait_for_output(text.decode(), 0.2):
            return
    raise AssertionError(f"{text!r} did not appear")


def children(pid: int) -> list[int]:
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        if int(stat[stat.rindex(")") + 2 :].split()[1]) == pid:
            found.append(int(entry.name))
    return found


def alive(pid: int) -> bool:
    try:
        state = Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return False
    return state[state.rindex(")") + 2] != "Z"


def test_resize_inside_the_canvas_and_publish_the_capture(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = tt.record_original(["/bin/sh", "-c", LOOP], OPTIONS)
    assert original.capture_status()["state"] == "live"
    wait_for_output(original, b"ready")

    original.session().resize(100, 30)
    wait_for_output(original, b"winch 30 100")
    with pytest.raises(TermtraceError) as rejected:
        original.session().resize(400, 200)
    assert rejected.value.code == "exceeds_native_canvas"
    assert rejected.value.effect == {"kind": "not_applied"}

    result = original.finish(60_000)
    assert result["capture"]["provenance"] == {"kind": "original_execution"}
    assert result["completion"]["session_id"] == original.session_id
    assert result["canonical_success"] is False
    assert original.capture_status()["state"] == "published"
    assert not original.is_active()


def test_failing_application_still_publishes(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = tt.record_original(["/bin/sh", "-c", "echo failing; exit 7"], OPTIONS)

    result = original.wait_finished(60_000)
    assert result["completion"]["exit_code"] == 7
    assert result["canonical_success"] is False
    assert result["capture"]["product_kind"] == "video"


def test_killed_encoder_raises_with_canonical_completion(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = tt.record_original(["/bin/sh", "-c", LOOP], OPTIONS)
    supervisor = int(original.capture_status()["supervisor_pid"])
    encoders = [
        pid
        for helper in children(supervisor)
        for pid in children(helper)
        if Path(f"/proc/{pid}/comm").read_text().startswith("ffmpeg")
    ]
    assert len(encoders) == 1
    os.kill(encoders[0], signal.SIGKILL)

    with pytest.raises(OriginalCaptureFailedError) as failed:
        original.finish(60_000)
    assert failed.value.code == "acquisition_failed"
    assert failed.value.completion is not None
    assert failed.value.scope == "management"
    assert failed.value.effect is None
    assert any(item["kind"] == "session" for item in failed.value.evidence)
    assert not alive(supervisor)


def test_conflicting_geometry_is_rejected(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    with pytest.raises(TermtraceError) as conflict:
        tt.record_original(
            ["/bin/true"],
            {
                "grid": {"cols": 80, "rows": 24},
                "viewport": {"width": 800, "height": 600},
            },
        )
    assert conflict.value.kind == "invalid_operation"
    with pytest.raises(ValueError, match="rows"):
        tt.record_original(["/bin/true"], {"rows": 24})  # type: ignore[typeddict-unknown-key]


@pytest.mark.asyncio
async def test_cancelled_async_start_tears_the_capture_down(tmp_path: Path) -> None:
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    token = CancelToken()
    start = asyncio.create_task(
        tt.record_original(["/bin/sh", "-c", "sleep 30"], OPTIONS, token)
    )
    deadline = time.monotonic() + 30
    while not list(tmp_path.glob("*.original-capture.json")):
        assert time.monotonic() < deadline, "acquisition never started"
        await asyncio.sleep(0.01)
    start.cancel()

    with pytest.raises(asyncio.CancelledError):
        await start
    assert token.cancelled
    sidecar = next(tmp_path.glob("*.original-capture.json"))
    supervisor = int(sidecar.read_text().split('"supervisor_pid": "')[1].split('"')[0])
    assert not alive(supervisor)


@pytest.mark.asyncio
async def test_async_finish_returns_the_published_capture(tmp_path: Path) -> None:
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = await tt.record_original(["/bin/sh", "-c", "echo done"], OPTIONS)

    result = await original.wait_finished(60_000)
    assert result["canonical_success"] is True
    assert (await original.capture_status())["state"] == "published"


def start_named(state: Path, session: bytes, alias: str, script: str = LOOP) -> None:
    subprocess.run(
        [
            os.fsencode(TERMTRACE_BIN),
            b"record",
            b"--original-native",
            b"--original-grid",
            b"80x24",
            b"--original-canvas",
            b"1280x720",
            b"-d",
            alias.encode(),
            b"--out",
            session,
            b"--",
            b"/bin/sh",
            b"-c",
            script.encode(),
        ],
        check=True,
        env={**os.environ, "TERMTRACE_STATE_DIR": str(state)},
        timeout=60,
    )


def test_named_background_recording_attaches_by_alias(tmp_path: Path) -> None:
    state = tmp_path / "state"
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "evidence"))
    started = tt.record_original(
        ["/bin/sh", "-c", LOOP],
        {**OPTIONS, "alias": "py-attach", "state_root": str(state)},
    )
    session_id = started.session_id
    started.detach()

    named = tt.attach_original_target("name:py-attach", state_root=state)
    assert named.session_id == session_id
    status = named.capture_status()
    assert status["state"] == "live"
    assert isinstance(status["supervisor_pid"], int)
    wait_for_output(named, b"ready")
    named.session().resize(100, 30)
    wait_for_output(named, b"winch 30 100")

    by_path = tt.attach_original_target(Path(named.path))
    assert by_path.path_bytes == named.path_bytes
    with pytest.raises(TermtraceError):
        tt.attach_original_target("name:py-missing", state_root=os.fsencode(state))

    result = named.finish(60_000)
    assert result["completion"]["session_id"] == session_id
    assert result["capture"]["provenance"] == {"kind": "original_execution"}
    assert by_path.capture_status()["state"] == "published"


@pytest.mark.asyncio
async def test_async_attach_by_alias_controls_and_finishes(tmp_path: Path) -> None:
    state = tmp_path / "state"
    start_named(
        state,
        os.fsencode(tmp_path / "async-named"),
        "py-async",
        "echo ready; while read line; do echo got $line; done",
    )
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))

    named = await tt.attach_original_target("py-async", state_root=str(state))
    assert (await named.capture_status())["state"] == "live"
    session = named.session()
    await session.output().wait_for(b"ready", 20.0)
    await session.actions().send_text("hello\n").run()
    await session.output().wait_for(b"got hello", 20.0)

    result = await named.finish(60_000)
    assert result["capture"]["source"]["session_id"] == named.session_id


CONTROL_LOOP = (
    "(trap 'echo winch $(stty size </dev/tty)' WINCH; while :; do sleep 0.05; done) & "
    "echo ready; while read line; do echo got $line; done"
)


def start_non_utf8(tmp_path: Path, alias: str) -> tuple[Path, bytes]:
    state = tmp_path / "state"
    root = os.fsencode(tmp_path) + b"/evidence-\xff"
    os.mkdir(root)
    session = root + b"/session"
    start_named(state, session, alias, CONTROL_LOOP)
    return state, session


def test_exact_byte_path_attach_controls_a_non_utf8_session(tmp_path: Path) -> None:
    state, session_bytes = start_non_utf8(tmp_path, "py-bytes")
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))

    exact = tt.attach_original_target(session_bytes)
    assert exact.path_bytes == session_bytes
    assert exact.capture_status()["state"] == "live"
    session = exact.session()
    assert session.path_bytes == session_bytes
    screen = session.screen()
    presented = screen.wait_for_text("ready", timeout_secs=20.0)

    actions = session.actions()
    actions.if_state(presented.hash)
    actions.send_text("hello")
    actions.send_key("Enter")
    actions.until_screen("got hello", timeout_secs=20.0)
    assert "got hello" in actions.execute().text

    session.resize(100, 30)
    screen.wait_for_text("winch 30 100", timeout_secs=20.0)
    named = tt.attach_original_target("name:py-bytes", state_root=state)
    assert named.path_bytes == session_bytes
    assert named.session_id == exact.session_id

    result = exact.finish(60_000)
    assert result["capture"]["directory_bytes"].startswith(session_bytes + b"/")
    assert named.capture_status()["state"] == "published"


@pytest.mark.asyncio
async def test_async_exact_byte_path_attach_controls_a_non_utf8_session(
    tmp_path: Path,
) -> None:
    _, session_bytes = start_non_utf8(tmp_path, "py-async-bytes")
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))

    exact = await tt.attach_original_target(session_bytes)
    session = exact.session()
    assert session.path_bytes == session_bytes
    screen = session.screen()
    presented = await screen.wait_for_text("ready", 20.0)

    await (
        session.actions()
        .if_state(presented.hash)
        .send_text("hello")
        .send_key("Enter")
        .until_screen("got hello", timeout_secs=20.0)
        .run()
    )
    assert "got hello" in await screen.text()

    await session.resize(100, 30)
    await screen.wait_for_text("winch 30 100", 20.0)

    result = await exact.finish(60_000)
    assert result["capture"]["source"]["session_id"] == exact.session_id
    assert result["capture"]["directory_bytes"].startswith(session_bytes + b"/")


def rewrite_status(sidecar: Path, status: dict[str, object]) -> None:
    staged = sidecar.with_name(sidecar.name + ".rewrite")
    descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as file:
        json.dump(status, file)
    os.replace(staged, sidecar)


def test_capture_status_is_exact_native_integers(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = tt.record_original(["/bin/sh", "-c", "echo done"], OPTIONS)
    original.wait_finished(60_000)
    sidecar = Path(os.fsdecode(original.path_bytes + b".original-capture.json"))
    published = json.loads(sidecar.read_text())

    wide = json.loads(json.dumps(published))
    wide["supervisor_pid"] = str(2**64 - 1)
    wide["supervisor_start_ticks"] = str(2**53 + 1)
    wide["result"]["source"]["boundary"]["output_bytes"] = str(2**64 - 1)
    wide["result"]["members"][0]["byte_length"] = str(2**53 + 1)
    rewrite_status(sidecar, wide)
    status = original.capture_status()
    assert status["supervisor_pid"] == 2**64 - 1
    assert status["supervisor_start_ticks"] == 2**53 + 1
    assert type(status["supervisor_pid"]) is int
    result = status["result"]
    assert result is not None
    assert result["source"]["boundary"]["output_bytes"] == 2**64 - 1
    assert result["members"][0]["byte_length"] == 2**53 + 1
    assert result["members"][0]["schema_type"] is None
    assert isinstance(result["directory_bytes"], bytes)

    failed = json.loads(json.dumps(published))
    failed.update(
        state="failed",
        result=None,
        failure={
            "code": "recording_failed",
            "reason": str(2**64 - 1),
            "surviving": [],
            "cleanup": None,
        },
    )
    rewrite_status(sidecar, failed)
    status = original.capture_status()
    assert status["result"] is None
    failure = status["failure"]
    assert failure is not None
    assert failure["reason"] == str(2**64 - 1)
    assert failure["cleanup"] is None

    for bad in ["01", "-1", str(2**64), 7]:
        malformed = json.loads(json.dumps(published))
        malformed["supervisor_pid"] = bad
        rewrite_status(sidecar, malformed)
        with pytest.raises(TermtraceError):
            original.capture_status()


def parent_of(pid: int) -> int:
    stat = Path(f"/proc/{pid}/stat").read_text()
    return int(stat[stat.rindex(")") + 2 :].split()[1])


def replace_status(session: bytes, status: bytes) -> None:
    sidecar = session + b".original-capture.json"
    staged = sidecar + b".rewrite"
    descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as file:
        file.write(status)
    os.replace(staged, sidecar)


def test_swapped_sidecar_after_attach_is_a_capture_failure(tmp_path: Path) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    first = tt.record_original(["/bin/sh", "-c", "echo first"], OPTIONS)
    first.wait_finished(60_000)
    second = tt.record_original(["/bin/sh", "-c", "echo second"], OPTIONS)
    second.wait_finished(60_000)
    other = Path(os.fsdecode(first.path_bytes + b".original-capture.json")).read_bytes()
    own = Path(os.fsdecode(second.path_bytes + b".original-capture.json")).read_bytes()

    attached = tt.attach_original_target(second.path_bytes)
    replace_status(second.path_bytes, other)
    with pytest.raises(TermtraceError):
        attached.capture_status()
    with pytest.raises(OriginalCaptureFailedError) as swapped:
        attached.wait_finished(60_000)
    assert swapped.value.code == "publication_failed"
    assert swapped.value.completion is not None
    assert swapped.value.completion["session_id"] == second.session_id

    redirected = json.loads(own)
    redirected["result"]["directory_bytes"] = json.loads(other)["result"][
        "directory_bytes"
    ]
    replace_status(second.path_bytes, own)
    attached = tt.attach_original_target(second.path_bytes)
    replace_status(second.path_bytes, json.dumps(redirected).encode())
    with pytest.raises(OriginalCaptureFailedError) as moved:
        attached.wait_finished(60_000)
    assert moved.value.code == "publication_failed"

    replace_status(second.path_bytes, own)
    result = tt.attach_original_target(second.path_bytes).wait_finished(60_000)
    assert result["capture"]["source"]["session_id"] == second.session_id


def test_owner_stops_hup_ignoring_application_after_supervisor_loss(
    tmp_path: Path,
) -> None:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path))
    original = tt.record_original(
        [
            "/bin/sh",
            "-c",
            "trap '' HUP TERM; echo stubborn; while :; do sleep 0.05; done",
        ],
        OPTIONS,
    )
    supervisor = original.capture_status()["supervisor_pid"]
    status = original.session().status()
    assert status["kind"] == "live"
    application = status["pid"]
    assert application is not None
    for pid in (supervisor, parent_of(application)):
        os.kill(pid, signal.SIGKILL)

    deadline = time.monotonic() + 15
    while alive(application):
        assert time.monotonic() < deadline, "the application outlived its supervisor"
        time.sleep(0.05)
    with pytest.raises(OriginalCaptureFailedError) as abandoned:
        original.wait_finished(60_000)
    assert abandoned.value.code == "supervisor_abandoned"
