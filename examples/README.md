# Termtrace examples

The demo exercises Termtrace through its public CLI and retains owner-only evidence under an ignored output directory. Generated sessions, raw terminal bytes, screenshots, and videos may contain sensitive data; do not publish them without inspection.

SDK versions of the same workflow, in Rust, Python, and TypeScript, are in [sdk/](sdk/README.md).

## Run it

```bash
examples/demo.sh
```

Pass a different destination to keep output elsewhere:

```bash
examples/demo.sh /tmp/my-termtrace-demo
```

The demo refuses to replace existing output. Its artifacts are written beneath `<destination>/showcase`.

The script builds the release binary unless `TERMTRACE_BIN` names one. It requires Cargo,
Python 3, jq, FFmpeg with FFprobe, libx264 and libvpx-vp9, Ghostty, Xvfb, `dbus-run-session`,
and the JetBrains Mono font.

## What it demonstrates

One recording covers the complete workflow:

- original Ghostty pixels captured while the showcase runs, with a fixed visual profile;
- true color, Unicode graphemes, synchronized output, alternate-screen transitions, and terminal queries;
- readiness, state-guarded input, markers, signals, waits, and graceful stop;
- an edited preview, MP4, GIF, and WebM rendered from that one recording;
- asciinema v2 export, torn-tail recovery, tamper rejection, and JSON Schema generation.

## Artifact layout

```text
demo-artifacts/showcase/
├── run/                     canonical session
│   ├── events.jsonl
│   └── visual-original/     original capture and its manifest
├── edit.json
├── preview/contact-sheet.png
├── showcase.mp4             each output keeps a .termtrace sidecar manifest
├── showcase.gif
├── showcase.webm
├── probe-{mp4,gif,webm}.json
├── session.cast
├── recovery-report.json
└── tamper-rejected.log
```
