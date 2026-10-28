"""Record the terminal showcase once with original pixels, then edit it into MP4, GIF and WebM.

Usage: `python demo.py OUTPUT_DIRECTORY` (the directory must not exist). Requires Ghostty, Xvfb,
dbus-run-session, FFmpeg and the JetBrains Mono font. Set `TERMTRACE_BIN` to the CLI to use.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import time
from pathlib import Path

from termtrace import Select, Session, Termtrace, types
from termtrace.editing import Edit

REPOSITORY = Path(__file__).resolve().parents[3]
SHOWCASE = REPOSITORY / "examples/apps/terminal_showcase.py"
PROFILE: types.VisualProfile = {
    "version": 1,
    "native": {"font_family": "JetBrains Mono", "font_size_points": "12", "dpi": 96},
}
TIMEOUT_SECS = 20.0
SIGNALS: list[tuple[types.ProcessSignal, str]] = [
    ("user1", "SIGUSR1"),
    ("user2", "SIGUSR2"),
    ("hangup", "SIGHUP"),
    ("interrupt", "SIGINT"),
]


def canonical_hashes(session: Path) -> dict[str, str]:
    files = [
        session / name for name in ("session.json", "events.jsonl", "completion.json")
    ]
    files += sorted((session / "visual-original").iterdir())
    return {
        str(path.relative_to(session)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in files
        if path.is_file()
    }


def drive(live: Session) -> None:
    screen = live.screen()
    ready = screen.wait_for_text("SHOWCASE_READY", timeout_secs=TIMEOUT_SECS)
    live.mark("showcase-ready")
    # The showcase reads its terminal raw, so the line ends with a newline, not Enter.
    (
        live.actions()
        .if_state(ready.hash)
        .send_text("continue-from-file\n")
        .until_screen("WAITING_FOR_SIGNALS", timeout_secs=TIMEOUT_SECS)
        .run()
    )
    for signal, name in SIGNALS:
        # Presentation pacing so each step stays on screen; waits below synchronize.
        time.sleep(0.4)
        live.signal(signal)
        screen.wait_for_text(f"SIGNAL:{name}", timeout_secs=TIMEOUT_SECS)
    time.sleep(0.4)
    live.mark("signals-observed")


def main(output: Path) -> None:
    output.mkdir(mode=0o700, parents=True)
    tt = Termtrace(binary=os.environ.get("TERMTRACE_BIN"), evidence_root=str(output))

    report = tt.preflight_visual_profile(PROFILE, cols=80, rows=24)
    native = report["native"]
    assert native is not None
    assert native["dpi"] == 96
    assert "JetBrains Mono" in native["discovered_font_families"]
    assert (
        native["geometry"]["required_cols"],
        native["geometry"]["required_rows"],
    ) == (80, 24)

    original = tt.record_original(
        [sys.executable, str(SHOWCASE)],
        {
            "grid": {"cols": 80, "rows": 24},
            "canvas": {"width": 1280, "height": 720},
            "fps": 24,
            "visual_profile": PROFILE,
            "startup_timeout_ms": 60_000,
        },
    )
    try:
        drive(original.session())
    except BaseException:
        original.force_close()
        raise

    recording = original.finish(60_000)
    capture = recording["capture"]
    assert capture["provenance"] == {"kind": "original_execution"}
    session = Path(original.path)
    manifest = json.loads((session / "session.json").read_text())
    assert manifest["visual_profile"]["profile_hash"] == report["profile_hash"]
    completion = json.loads((session / "completion.json").read_text())
    capture_manifest = json.loads(
        (session / "visual-original/manifest.json").read_text()
    )
    assert capture_manifest["capture_kind"] == "original_execution"
    assert capture_manifest["effective_config_hash"] == native["effective_config_hash"]
    geometry = capture_manifest["geometry_report"]
    assert (geometry["cols"], geometry["rows"]) == (80, 24)
    original_video_hash = next(
        member["blake3"]
        for member in capture_manifest["members"]
        if member["path"] == "session.mkv"
    )
    before = canonical_hashes(session)

    edit = (
        Edit()
        .clip(
            Select.marker("showcase-ready"),
            Select.marker("signals-observed"),
            speed=(1, 1),
            hold_ms=500,
            caption="State-guarded input and terminal signals",
        )
        .caption_layout("footer")
    )
    renders = output / "renders"
    renders.mkdir(mode=0o700)
    preview = tt.video_preview(
        session, edit, renders / "preview", renderer="original_execution", fps=24
    )
    assert preview["plan"]["source"]["kind"] == "native"
    for extension in ("mp4", "gif", "webm"):
        render = tt.video_render(
            session,
            edit,
            renders / f"demo.{extension}",
            renderer="original_execution",
            fps=24,
        )
        assert render["plan"]["edit_plan_hash"] == preview["plan"]["edit_plan_hash"]
        sidecar = json.loads((Path(render["sidecar"]) / "manifest.json").read_text())
        if extension == "mp4":
            # The MP4 sidecar names its source by path and hash; it must be the original capture.
            assert sidecar["plan"]["source"]["video_hash"] == original_video_hash
        else:
            artifact = render["artifact"]
            assert artifact is not None
            assert artifact["provenance"] == {
                "kind": "presentation_output",
                "source_kind": "original_execution",
            }
            source_hash = artifact["source"]["session_manifest_hash"]
            assert source_hash == completion["session_manifest_hash"]

    assert canonical_hashes(session) == before
    summary = {
        "session": str(session),
        "profile_hash": report["profile_hash"],
        "edit_plan_hash": preview["plan"]["edit_plan_hash"],
        "contact_sheet": preview["contact_sheet"],
        "outputs": [
            str(renders / f"demo.{extension}") for extension in ("mp4", "gif", "webm")
        ],
    }
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: demo.py OUTPUT_DIRECTORY")
    main(Path(sys.argv[1]))
