"""Native Ghostty replay screenshots and videos of completed sessions."""

from __future__ import annotations

import json
import os
import struct
import subprocess
from pathlib import Path

import pytest
from termtrace import Edit, Select, Termtrace, TermtraceError
from termtrace.aio import AsyncTermtrace

TERMTRACE_BIN = os.path.abspath(
    os.environ.get("TERMTRACE_BIN", "target/debug/termtrace")
)
SCRIPT = "printf FIRST; sleep 0.5; printf ' SECOND'; IFS= read -r line"


def png_size(path: Path) -> tuple[int, int]:
    header = path.read_bytes()[:24]
    assert header[:8] == b"\x89PNG\r\n\x1a\n"
    width, height = struct.unpack(">II", header[16:24])
    return width, height


@pytest.fixture
def completed(tmp_path: Path) -> tuple[Termtrace, Path, str, str]:
    tt = Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))
    with tt.record(["/bin/sh", "-c", SCRIPT], rows=6, cols=30) as rec:
        first = rec.screen().wait_for_text("FIRST", timeout_secs=5)
        rec.mark("start")
        second = rec.screen().wait_for_text("SECOND", timeout_secs=5)
        rec.mark("end")
        rec.stop()
    return tt, Path(rec.path), first.hash, second.hash


def test_screenshot_captures_the_selected_state_as_native_pixels(
    tmp_path: Path, completed: tuple[Termtrace, Path, str, str]
) -> None:
    tt, session, first, second = completed
    native = tt.native(session)

    selected = native.screenshot(
        tmp_path / "first", point={"selector": "state_hash", "state_hash": first}
    )
    latest = tt.attach(str(session)).native().screenshot(tmp_path / "latest")

    for capture, state in [(selected, first), (latest, second)]:
        directory = Path(capture["directory"])
        manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
        assert capture["video"] is None
        assert manifest["capture_kind"] == "screenshot"
        assert manifest["guarantee_level"] == "native_capture_environment"
        assert manifest["ghostty_version"].startswith("Ghostty")
        assert manifest["selected_state_hash"] == state
        assert (
            manifest["geometry_report"]["cols"],
            manifest["geometry_report"]["rows"],
        ) == (30, 6)
        assert png_size(Path(capture["screenshot"])) == (
            manifest["width"],
            manifest["height"],
        )
        image = next(
            m for m in manifest["members"] if m["relationship"] == "native_final_frame"
        )
        assert directory / image["path"] == Path(capture["screenshot"])
    assert (
        Path(selected["screenshot"]).read_bytes()
        != Path(latest["screenshot"]).read_bytes()
    )


async def test_screenshot_never_replaces_an_existing_destination(
    tmp_path: Path, completed: tuple[Termtrace, Path, str, str]
) -> None:
    _, session, _, _ = completed
    native = AsyncTermtrace(binary=TERMTRACE_BIN).native(session)
    first = await native.screenshot(tmp_path / "shot")
    before = {
        path.name: path.read_bytes() for path in Path(first["directory"]).iterdir()
    }

    with pytest.raises(TermtraceError):
        await native.screenshot(
            tmp_path / "shot", point={"selector": "marker", "label": "start"}
        )

    after = {
        path.name: path.read_bytes() for path in Path(first["directory"]).iterdir()
    }
    assert after == before


def test_native_renderer_replays_the_session_into_a_native_video(
    tmp_path: Path, completed: tuple[Termtrace, Path, str, str]
) -> None:
    tt, session, _, _ = completed
    out = tmp_path / "native.webm"
    edit = Edit().clip(Select.marker("start"), Select.marker("end"))

    render = tt.video_render(session, edit, out, renderer="native", fps=24)

    artifact = render["artifact"]
    assert artifact is not None
    assert artifact["provenance"]["source_kind"] == "native_replay"
    source = render["plan"]["source"]
    assert source["kind"] == "native"
    frames = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,nb_read_frames",
            "-of",
            "json",
            str(out),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    stream = json.loads(frames.stdout)["streams"][0]
    assert stream["codec_name"] == "vp9"
    assert int(stream["nb_read_frames"]) > 0
    capture = json.loads(
        (Path(source["video"]).parent / "manifest.json").read_text(encoding="utf-8")
    )
    assert capture["capture_kind"] == "video"
    assert capture["guarantee_level"] == "native_capture_environment"
