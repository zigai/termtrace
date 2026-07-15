# Termtrace demo

The demo exercises Termtrace through its public CLI and retains owner-only evidence under an ignored output directory. Generated sessions, raw terminal bytes, screenshots, and videos may contain sensitive data; do not publish them without inspection.

## Run it

```bash
demos/run-all.sh
```

Pass a different destination to keep output elsewhere:

```bash
demos/run-all.sh /tmp/my-termtrace-demo
```

The demo refuses to replace existing output. Its artifacts are written beneath `<destination>/showcase`.

## What it demonstrates

One recording covers the complete workflow:

- true color, Unicode graphemes, synchronized output, alternate-screen transitions, and terminal queries;
- readiness probes, status, exact file input, resizing, markers, signals, waits, and graceful stop;
- semantic state derivation, JSON and text inspection, and asciinema v2 export;
- a native screenshot, lossless 1920×1080 video at 24 FPS, and a broadly playable MP4;
- torn-tail recovery, recovered-prefix inspection, tamper rejection, and JSON Schema generation.

## Artifact layout

```text
demo-artifacts/showcase/
├── run/
│   ├── events.jsonl
│   ├── derived/states.jsonl.zst
│   ├── derived/state-index.jsonl
│   ├── visual/session.mkv
│   └── visual/final.png
├── final-state.json
├── final-screen.txt
├── session.cast
├── showcase.mp4
├── video-probe.json
├── event-counts.json
├── recovery-report.json
└── tamper-rejected.log
```
