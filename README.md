# Termtrace

Termtrace records terminal and TUI sessions so you can review what happened without relying on video alone. It preserves the original PTY event stream, reconstructs terminal states for automated inspection, and can render native screenshots and video.

## Features

- **Recording** — run commands interactively or in the background
- **Live control** — view the screen, wait for changes, and send input
- **Isolated launches** — clear/copy/set/unset environment policy, deterministic color behavior, secret-safe transport, and non-truncating output budgets
- **Replay** — inspect every terminal state, including brief changes
- **Review** — browse timelines, compare states, and diagnose rendering issues
- **Capture** — create text, JSON, HTML, screenshots, native or semantic video, and asciinema exports
- **Verification** — check integrity, recover damaged recordings, and detect changes
- **Automation** — run terminal tests from the CLI, Python, TypeScript, or Rust

## Install

Install the latest prebuilt Linux release without Rust, Zig, or a source checkout:

```bash
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
termtrace doctor --explain
```

The high-level [`install.sh`](install.sh) script verifies the release checksum and installs to `~/.local/bin` by default. For a stable, pinned installer, use the copy attached to a release. See [Installation](docs/installation.md) for pinned versions, private-repository access, manual verification, and source builds.

## Getting started

Record any command directly after the recording options:

```bash
termtrace record --out ./example.termtrace --geometry inherit your-command
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
termtrace start demo --cols 120 --rows 36 your-command
termtrace wait demo 'Ready' --stable-for 250ms
termtrace wait-idle demo --stable-for 250ms
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

When native display dependencies are unavailable, explicitly render the same presentation from deterministic semantic state instead:

```bash
termtrace video export ./example.termtrace \
  --source semantic \
  --edit ./edit.json \
  --out ./semantic-presentation
```

Native Ghostty pixels remain the default and the correct source for visual fidelity or renderer diagnostics. Semantic video is a portable generated visualization and never silently replaces native capture.

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
- [Deterministic test isolation](docs/test-isolation.md)
- [Control and inspect a running session](docs/agent-workflows.md)
- [Use the Python and TypeScript clients](docs/language-clients.md)
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

See [Installation](docs/installation.md#build-from-source) for contributor build requirements.
