# Termtrace

Termtrace records terminal and TUI sessions as two synchronized sources of evidence:

- native Ghostty screenshots and 24 FPS video for human visual review;
- a byte-exact event log replayed through `libghostty-vt` for agent-readable terminal states.

The raw event log is authoritative. Videos, screenshots, text, JSON, VT, and HTML views are derived artifacts that can be regenerated after a session exits.

> [!WARNING]
> Terminal recordings can contain prompts, source code, environment details, credentials, clipboard data, and typed secrets. Session directories are owner-only by default and must be treated as sensitive.

## Status

Termtrace is under active development. The architecture and acceptance criteria are documented in [Architecture](docs/architecture.md).

The functional PTY proxy supports both interactive and non-interactive commands:

```bash
cargo run -- record \
  --out /tmp/example.termtrace \
  --geometry fit --viewport 1920x1080 \
  --ghostty-config ./recording.ghostty \
  --font-size 16 --padding-x 8 --padding-y 8 \
  -- your-command --argument
```

Fitted geometry launches a short Ghostty probe on the requested viewport and measures the actual grid, text area, and cell size from the configured font, cell adjustments, padding, and display environment. The measured values and Ghostty configuration hash are retained in `session.json`. Ghostty and Xvfb are required for this mode.

`--ghostty-config PATH` uses that file instead of the normal user configuration. A focused set of convenience flags is applied after the file and therefore wins: `--font-family`, `--font-size`, `--padding-x`, `--padding-y`, `--foreground`, and `--background`. Without `--ghostty-config`, fitted geometry resolves the current user configuration as its base. Termtrace flattens the result into `ghostty-config.txt` in the session and uses that same hashed profile for geometry probing, terminal colors, and later native capture. The source config may subsequently move or change; external resources such as the selected font or custom shader are referenced, not bundled.

Manual geometry remains available when native capture is not planned:

```bash
termtrace record \
  --out /tmp/example.termtrace \
  --geometry manual \
  --cols 160 --rows 40 \
  --cell-width 9 --cell-height 18 \
  -- your-command --argument
```

Use `--geometry inherit` to copy the controlling terminal's rows and columns. Kernel-reported pixel dimensions are preferred; `--cell-width` and `--cell-height` are deterministic fallbacks when the terminal reports only its grid.

Command output is mirrored unchanged. Standard input is forwarded in raw mode, outer-terminal resizes are propagated, and host terminal settings are restored on every normal and error path. Use `--no-input` for a detached producer. The child receives a real PTY with `TERM=xterm-ghostty` and `COLORTERM=truecolor`; the command's exit status is propagated.

The child inherits the recorder's current working directory by default, so `cd project && termtrace record ... -- cargo test` runs in `project`. `--cwd PATH` is an explicit override; relative paths are resolved from the recorder's current directory. The resolved effective directory is stored in `session.json` as byte-exact provenance.

The new owner-only session directory contains an integrity-chained `events.jsonl` recording with raw bytes encoded as Base64. Inputs and resizes are represented by durable intent/result pairs. A crash between the pair is reported as uncertain, never silently retried. High-volume output is buffered between durable control boundaries so recording does not add an `fsync` pause to every PTY read. A live pinned-libghostty engine answers terminal queries and audits each generated reply as distinct host input. The header also pins Ghostty's effective foreground, pure-black background, cursor color, and complete palette for deterministic replay.

Termtrace refuses to reuse an existing session directory. This prevents accidental evidence loss and makes every run independently auditable.

Detailed references: [installation](docs/installation.md), [agent workflows](docs/agent-workflows.md), [semantic replay](docs/semantic-replay.md), [formats and schemas](docs/formats.md), and [troubleshooting](docs/troubleshooting.md).

## Reproducible demos

Run `demos/run-all.sh` to generate three owner-only showcases covering visual/semantic fidelity, live agent control and signals, tmux interoperability, schema generation, tamper rejection, and torn-tail recovery. Each visual demo includes the canonical session, lossless 1080p/24 FPS Ghostty capture, screenshot, agent-readable states, asciicast export, and a broadly playable MP4. See [the demo guide](demos/README.md) for individual commands and artifact layout.

## Live agent control

Every active recording exposes an owner-only `control.sock` inside its session directory. Requests use bounded, newline-framed, versioned JSON and Linux peer credentials must match the recording owner. The CLI is a thin client:

```bash
termtrace status --session /tmp/example.termtrace
termtrace send --session /tmp/example.termtrace \
  --text $'inspect src\n' --intent 'run repository inspection'
termtrace resize --session /tmp/example.termtrace --cols 120 --rows 36
termtrace mark --session /tmp/example.termtrace --label before-build
termtrace signal --session /tmp/example.termtrace --signal interrupt
termtrace stop --session /tmp/example.termtrace
```

Use `termtrace send --file PATH` for arbitrary bytes. The Rust API also exposes `control::send_request`; retain and resend the same request UUID after a transport failure. The session actor returns the cached result and never applies that mutation twice. Reusing a UUID for a different command is rejected.

The socket disappears when the child exits. Completed-session inspection reads the canonical log rather than pretending that a dead control endpoint is live.

## Semantic replay

```bash
termtrace index --session /tmp/example.termtrace
```

Replay validates the event hash chain, feeds each output byte through the pinned libghostty-vt parser, and stores every distinct state under `derived/`. Synchronized-output intermediate states remain indexed with `presented: false`; video frame rate never limits semantic evidence.

- `states.jsonl` contains cursor, modes, colors, active SGR style, OSC metadata, Ghostty dirty rows, and cell-level grapheme/style/color diffs.
- `state-index.jsonl` maps state hashes to event sequence, byte offset, monotonic time, and presentation status.
- `final.txt`, `final.vt`, and `final.html` come from Ghostty's formatter.
- `manifest.json` pins the source event hash, schema version, Ghostty revision, artifact paths, and final state hash.

The derived directory is assembled privately and atomically renamed with no-replace semantics. Existing results are never overwritten implicitly.

Index only a half-open source interval when a long recording needs focused analysis:

```bash
termtrace index --session /tmp/example.termtrace \
  --from 00:08:15 --to 00:08:45
```

`--from` is inclusive and `--to` is exclusive. Offsets accept bare seconds, `ns`/`us`/`ms`/`s`/`m`/`h` suffixes, `MM:SS.fraction`, or `HH:MM:SS.fraction`. An omitted bound selects the corresponding session endpoint. Termtrace fast-forwards all earlier output and successful resizes through libghostty-vt, emits a complete baseline at `--from`, and then indexes only states in the selected interval. Range artifacts use deterministic `derived-range-FROM_NS-TO_NS/` directories and coexist with `derived/`.

## Native Ghostty pixels

```bash
termtrace capture --session /tmp/example.termtrace --fps 24
```

Capture uses the viewport retained by `--viewport`; recordings without one retain the 1280x720 default. Supplying both `--width` and `--height` overrides it. Before event time zero, playback forces the recorded initial grid and verifies Ghostty's live geometry. A fitted recording also verifies the measured text-area pixels and effective configuration hash, failing instead of silently changing wrapping.

Capture launches the installed Ghostty GTK/OpenGL application on a private Xvfb display. Recordings made with fitted geometry or any visual-profile option use the canonical `ghostty-config.txt` saved in the session, so settings such as background, font, palette, padding, and shader choices are evidence rather than assumptions. Older and manual recordings without a saved profile retain the normal user-config behavior. Capture-only command-line overrides isolate the process, disable single-instance behavior, position and expand the window to the capture surface, and prevent persisted window state from changing the result.

`visual/session.mkv` is lossless RGB H.264 at exactly 24 FPS. `visual/final.png` is a direct X11 frame with the mouse hidden. FFprobe output, process logs, config, provenance, and `video-map.jsonl` are retained beside them. Failed captures preserve their owner-only diagnostic directory instead of deleting the evidence.

Native video is always an offline derived artifact; recording never encodes video continuously. To render only a suspicious interval, first create its matching range index and then capture with the same bounds:

```bash
termtrace index   --session /tmp/example.termtrace --from 00:08:15 --to 00:08:45
termtrace capture --session /tmp/example.termtrace --from 00:08:15 --to 00:08:45
```

Compound unit strings such as `8m15s` are not accepted; use clock notation (`00:08:15`) or one decimal unit (`495s`). During ranged capture, canonical output and successful resizes before `--from` are replayed without source-time sleeps. A terminal-query barrier proves Ghostty processed that prefix before the capture clock and FFmpeg start, then only `[from, to)` is played at native timing. The manifest records the exact source range and `video-map.jsonl` maps absolute source timestamps to range-relative frames. Range captures use `visual-range-FROM_NS-TO_NS/`; omitting both bounds preserves `visual/` and full-session timing.

Run `scripts/native-smoke-test.sh` on a Linux host with Ghostty, Xvfb, FFmpeg/FFprobe, D-Bus, and `jq` to exercise and probe the complete native path.

## Development

The repository pins Rust, Ghostty, and Zig. Bootstrap a fresh clone and validate a change with:

```bash
scripts/bootstrap.sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
```

The bootstrap initializes the Ghostty submodule, downloads Zig 0.15.2 from ziglang.org, verifies its pinned SHA-256, and builds Ghostty revision `55a3e33ab26a23d75b274b23c7f76d837db00578`. Checked-in bindings keep bindgen and libclang out of normal builds and remain private behind the safe `ghostty_vt` adapter. They are regenerated explicitly only during a pinned Ghostty upgrade. Set `TERMTRACE_GHOSTTY_PREFIX` only when intentionally testing an equivalent external libghostty-vt installation.
