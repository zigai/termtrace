# Termtrace

Termtrace records terminal and TUI sessions so you can review what happened without relying on video alone. It preserves the original PTY event stream, reconstructs terminal states for automated inspection, and can render native screenshots and video.

## Features

- **Recording** — run commands interactively or in the background
- **Live control** — view the screen, wait for changes, and send input
- **Replay** — inspect every terminal state, including brief changes
- **Review** — browse timelines, compare states, and diagnose rendering issues
- **Capture** — create text, JSON, HTML, screenshots, video, and asciinema exports
- **Verification** — check integrity, recover damaged recordings, and detect changes
- **Automation** — run terminal tests from the CLI, Python, TypeScript, or Rust

## Getting started

Bootstrap the native dependency, build the release binary, and check what your machine supports:

```bash
scripts/bootstrap.sh
cargo build --release
target/release/termtrace doctor --explain
```

Record any command by placing it after `--`:

```bash
termtrace record --out ./example.termtrace --geometry inherit -- your-command
```

The command runs in a real PTY. Its output stays visible and input is forwarded normally. When it exits, you can inspect the final screen, browse the timeline, or verify the recording:

```bash
termtrace show ./example.termtrace
termtrace timeline ./example.termtrace
termtrace verify ./example.termtrace --level content
```

Every recording needs a new output path. Existing session directories are never replaced.

## Working with a live session

Use a name when you want to start a command in the background and control it from another process:

```bash
termtrace start demo --cols 120 --rows 36 -- your-command
termtrace wait demo 'Ready' --stable-for 250ms
termtrace send demo text:'help' enter
termtrace show demo
termtrace stop demo --wait
```

`start` prints the canonical session path. The name is a local shortcut that can be listed, resolved, or removed without changing the recording.

## Reviewing a recording

Semantic replay lets you inspect states that may have existed for less than a video frame:

```bash
termtrace index --session ./example.termtrace
termtrace show ./example.termtrace --at 5s
termtrace diagnose ./example.termtrace
```

Create a deterministic semantic screenshot:

```bash
termtrace screenshot ./example.termtrace --out ./example-screenshot
```

Or capture native terminal pixels on a private display:

```bash
termtrace capture --session ./example.termtrace
```

Native capture is an optional capability. `termtrace doctor --explain` reports any missing display or media dependencies.

## See it in action

The demo walks through recording, live control, semantic replay, native capture, recovery, and tamper detection:

```bash
demos/run-all.sh
```

Its output includes one complete session, semantic states, a screenshot, native video, and verification reports. See the [demo guide](demos/README.md) for the artifact layout.

## How evidence is organized

The byte-exact event log is canonical. It records PTY output alongside input, resize, signal, marker, lifecycle, and exit events. Semantic states, text views, screenshots, videos, and exports are derived artifacts that can be regenerated from the recording.

## Documentation

- [Install Termtrace](docs/installation.md)
- [Record a session](docs/recording.md)
- [Control and inspect a running session](docs/agent-workflows.md)
- [Understand replay](docs/semantic-replay.md)
- [Understand the saved files](docs/formats.md)
- [Fix common problems](docs/troubleshooting.md)

For the complete command reference, use `termtrace --help` or `termtrace COMMAND --help`.

## Development

Termtrace is under active development and currently targets Linux.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

See [Installation](docs/installation.md#build-from-source) for build requirements.
