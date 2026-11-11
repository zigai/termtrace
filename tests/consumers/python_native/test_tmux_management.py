"""Real private-socket aggregate management through native services, never a CLI JSON facade."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path

import pytest
from termtrace import TermtraceError, Tmux
from test_evidence_pages import BINARY, evidence_root


def wait_until(operation, predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while True:
        try:
            result = operation()
        except TermtraceError:
            # A new private server creates its socket after record returns.
            if time.monotonic() >= deadline:
                raise
        else:
            if predicate(result) or time.monotonic() >= deadline:
                break
        time.sleep(0.025)
    assert predicate(result), result
    return result


def tmux(
    socket: Path, *arguments: str, check=True
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["/usr/bin/tmux", "-S", str(socket), *arguments],
        capture_output=True,
        check=check,
    )


# A Unix socket path holds at most 107 bytes, which pytest's per-worker temporary paths exceed.
@pytest.fixture
def socket_directory() -> Iterator[Path]:
    with tempfile.TemporaryDirectory(prefix="tt-") as directory:
        yield Path(directory)


def exercise_native_lifecycle(
    root: Path, socket_directory: Path, *, bundle_checks: bool
) -> None:
    socket = socket_directory / "s"
    # A selected helper alias (not the Python host) and quoted output-root exercise routing/quoting.
    helper = root / "selected ' Termtrace"
    helper.symlink_to(Path(BINARY).resolve())
    state = root / "state ' quoted"
    api = Tmux(
        root=os.fsencode(state),
        binary=os.fsencode(helper),
        tmux=b"/usr/bin/tmux",
        socket=os.fsencode(socket),
    )
    started = False
    try:
        report = api.record(
            {
                "name": "native",
                "new_server": True,
                "ghostty_profile": "off",
                "command": [b"/usr/bin/tmux", b"new-session", b"-s", b"native"],
            }
        )
        started = True
        assert isinstance(report["manifest"], bytes)
        audit = wait_until(api.status, lambda value: value["recorded_panes"] >= 1)
        assert audit["configured_executable"] == os.fsencode(Path(BINARY).resolve())
        assert audit["enabled"] and audit["default_command_matches"]
        tracks = wait_until(
            lambda: api.catalog("native"), lambda value: len(value["entries"]) >= 1
        )
        entry = tracks["entries"][0]
        assert tracks["kind"] == "tracks"
        assert isinstance(entry["track"]["session"], bytes)
        assert isinstance(entry["track"]["linked_unix_ns"], int)
        api.track(
            report["manifest"], entry["track"]["pane_id"], entry["track"]["session"]
        )
        with pytest.raises(TermtraceError):
            api.track(report["manifest"], "%987654321", entry["track"]["session"])
        with pytest.raises(TermtraceError):
            api.track(
                report["manifest"],
                entry["track"]["pane_id"],
                os.fsencode(root / "missing-source"),
            )
        gaps = api.catalog("native", gaps=True)
        assert gaps["kind"] == "gaps" and gaps["entries"]
        history = api.events("native", limit=1000)
        assert history["page"]["events"]
        ended = api.stop("native", timeout_ms=10000)
        assert ended["lifecycle"] == "completed"
        again = api.stop("native", timeout_ms=10000)
        assert again["lifecycle_journal"] == ended["lifecycle_journal"]
        closed = api.catalog("native")
        assert closed["summary"]["unfinished_tracks"] == 0
        assert all(item["completed"] for item in closed["entries"])
        if bundle_checks:
            destination = os.fsencode(root / "bundle")
            bundle = api.bundle(report["manifest"], destination)
            assert bundle["track_count"] >= 1
            assert api.verify_bundle(report["manifest"], destination) == bundle
            with pytest.raises(TermtraceError):
                api.bundle(report["manifest"], destination)
            # An independently altered aggregate identity cannot authorize this bundle.
            source_dir = Path(os.fsdecode(report["manifest"])).parent
            wrong = root / "mismatch" / "native"
            shutil.copytree(source_dir, wrong)
            manifest = json.loads((wrong / "manifest.json").read_bytes())
            manifest["recording_id"] = "00000000-0000-0000-0000-000000000000"
            (wrong / "manifest.json").write_text(json.dumps(manifest))
            with pytest.raises(TermtraceError):
                api.verify_bundle(os.fsencode(wrong / "manifest.json"), destination)
        (root / "tmux-observed.json").write_text(
            json.dumps(
                {
                    "selected_helper_hex": audit["configured_executable"].hex(),
                    "track_count": len(closed["entries"]),
                    "gap_count": len(gaps["entries"]),
                    "lifecycle": ended["lifecycle"],
                    "manifest_sha256": hashlib.sha256(
                        Path(os.fsdecode(report["manifest"])).read_bytes()
                    ).hexdigest(),
                },
                indent=2,
            )
        )
    finally:
        if started:
            api.stop("native", timeout_ms=10000)
        tmux(socket, "kill-server", check=False)
    assert tmux(socket, "list-sessions", check=False).returncode != 0


def test_private_tmux_native_lifecycle_helper(
    tmp_path: Path, socket_directory: Path
) -> None:
    exercise_native_lifecycle(
        evidence_root(tmp_path), socket_directory, bundle_checks=False
    )


def test_private_tmux_native_bundle(tmp_path: Path, socket_directory: Path) -> None:
    exercise_native_lifecycle(
        evidence_root(tmp_path), socket_directory, bundle_checks=True
    )


def test_existing_private_server_rolls_back_failed_composite_start(
    tmp_path: Path,
    socket_directory: Path,
) -> None:
    root = evidence_root(tmp_path)
    socket = socket_directory / "r"
    tmux(socket, "-f", "/dev/null", "new-session", "-d", "-s", "external")
    original = tmux(socket, "show-option", "-gv", "default-command").stdout
    api = Tmux(
        root=os.fsencode(root / "state"),
        binary=b"/bin/false",
        tmux=b"/usr/bin/tmux",
        socket=os.fsencode(socket),
    )
    try:
        with pytest.raises(TermtraceError):
            api.record(
                {
                    "name": "failed",
                    "ghostty_profile": "off",
                    "command": [
                        b"/usr/bin/tmux",
                        b"attach-session",
                        b"-t",
                        b"external",
                    ],
                }
            )
        assert tmux(socket, "show-option", "-gv", "default-command").stdout == original
        assert not tmux(
            socket, "show-option", "-gqv", "@termtrace-recording-version"
        ).stdout.strip()
        assert not (root / "state/tmux-recordings/failed").exists()
        tmux(socket, "has-session", "-t", "external")
    finally:
        tmux(socket, "kill-server", check=False)


def test_nonutf8_typed_pane_map_and_partially_effected_stop(
    tmp_path: Path, socket_directory: Path
) -> None:
    import signal

    from termtrace import Registry

    root = evidence_root(tmp_path)
    socket = socket_directory / "s"
    state = os.fsencode(root) + b"/state-\xff"
    api = Tmux(
        root=state,
        binary=os.fsencode(BINARY),
        tmux=b"/usr/bin/tmux",
        socket=os.fsencode(socket),
    )
    paused = None
    try:
        start = api.record(
            {
                "name": "partial",
                "new_server": True,
                "ghostty_profile": "off",
                "command": [b"/usr/bin/tmux", b"new-session", b"-s", b"native"],
            }
        )
        wait_until(api.status, lambda v: v["recorded_panes"] >= 1)
        manifest = wait_until(
            lambda: api.recording_status("partial"),
            lambda v: bool(v["pane_tracks"]),
        )
        assert all(
            isinstance(p, bytes) and b"\xff" in p
            for p in manifest["pane_tracks"].values()
        )
        for p in start["recording"]["pane_tracks"].values():
            assert isinstance(p, bytes) and b"\xff" in p
        gaps = api.catalog("partial", gaps=True)
        assert any(v["gap"]["resolved_unix_ns"] is None for v in gaps["entries"])
        entry = Registry(state, os.fsencode(BINARY)).resolve("tmux-partial-composite")
        paused = entry["recorder_pid"]
        assert paused is not None
        os.kill(paused, signal.SIGSTOP)
        with pytest.raises(TermtraceError) as caught:
            api.stop("partial", timeout_ms=30)
        error = caught.value
        assert (
            error.scope == "management" and error.effect is None and error.retry is None
        )
        assert error.guidance == "inspect_no_automatic_retry"
        assert error.management["operation"] == "tmux"
        facts = error.management["facts"]
        assert facts["stop_requested"] is True and facts["phase"] == "stop_requested"
        assert facts["manifest"] == start["manifest"]
        assert facts["completed_track_stops"] == []
        assert error.kind == "control_timeout"
    finally:
        if paused is not None:
            os.kill(paused, signal.SIGCONT)
        try:
            api.stop("partial", timeout_ms=10000)
        finally:
            tmux(socket, "kill-server", check=False)
