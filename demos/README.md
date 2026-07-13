# Termtrace demos

These demos exercise Termtrace through its public CLI and retain owner-only evidence under an ignored output directory. Generated sessions, raw terminal bytes, screenshots, and videos may contain sensitive data; do not publish them without inspection.

## Run everything

```bash
demos/run-all.sh
```

Pass an empty destination to keep output elsewhere:

```bash
demos/run-all.sh /tmp/my-termtrace-demos
```

Every demo refuses to replace existing output.

## 1. Visual semantics

`01-visual-semantics.sh` records an interactive terminal target with true color, Unicode graphemes, synchronized output, alternate-screen transitions, cursor and background queries, exact file input, resize, markers, waits, and `SIGUSR1`. It then produces:

- canonical byte and control events;
- every distinct libghostty state and state index;
- JSON and text inspection at initial and final states;
- an asciinema v2 cast;
- a real Ghostty/Xvfb screenshot;
- a lossless RGB H.264 video at 24 FPS;
- a broadly playable H.264 MP4 showcase derived from that lossless source;
- FFprobe and event-count summaries.

## 2. Control lifecycle

`02-control-lifecycle.sh` demonstrates readiness probes, status, textual input, waits, markers, `SIGUSR2`, `SIGHUP`, `SIGINT`, and graceful `stop`/`SIGTERM`. The target prints each observed transition, and the completed recording receives the same replay, inspection, cast, screenshot, and video derivations.

## 3. tmux, integrity, and recovery

`03-tmux-recovery.sh` records tmux as an ordinary target while Termtrace remains the canonical outer-PTY owner. It then:

- builds semantic and native visual artifacts;
- exports asciicast interoperability output;
- appends a torn JSON tail and recovers the exact valid prefix;
- indexes and inspects the recovered recording;
- alters canonical output without updating the hash chain and proves indexing rejects it;
- generates every public JSON Schema.

## Artifact layout

Each visual demo places its principal outputs at:

```text
demo-artifacts/<demo>/
├── run/
│   ├── events.jsonl
│   ├── derived/states.jsonl
│   ├── derived/state-index.jsonl
│   ├── visual/session.mkv
│   └── visual/final.png
├── final-state.json
├── final-screen.txt
├── session.cast
├── showcase.mp4
├── video-probe.json
└── event-counts.json
```

## Resolution

All native captures and showcase MP4s are generated at **1920×1080 and 24 FPS**.
