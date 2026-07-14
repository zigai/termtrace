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

The new owner-only session directory contains an integrity-chained `events.jsonl` recording with raw bytes encoded as Base64. Inputs and resizes are represented by durable intent/result pairs. A crash between the pair is reported as uncertain, never silently retried. A separate hash-chained `requests.jsonl` journal preserves exact mutation outcomes so retries remain idempotent after the bounded in-memory cache evicts an entry. Output persistence is selectable with `--durability strict|bounded|throughput`; bounded is the default at 50 ms or 65,536 pending PTY bytes and its thresholds can be tuned with `--durability-max-interval-ms` and `--durability-max-uncommitted-bytes`. A live pinned-libghostty engine answers terminal queries and audits each generated reply as distinct host input. The header also pins Ghostty's effective foreground, pure-black background, cursor color, and complete palette for deterministic replay.

Execution context uses an explicit `--provenance minimal|reproducible|custom` policy. The default retains terminal values while inventorying locale and timezone names; reproducible mode retains their exact values and hashes. Reproducible mode also hashes the resolved executable, recorder binary, and a directly invoked interpreter script when it can be identified without guessing. Custom rules such as `--provenance-env API_TOKEN=hash` or `--provenance-env SECRET=not-captured` distinguish equality evidence from reproducible values without silently treating hashes as secret-safe.

Termtrace refuses to reuse an existing session directory. This prevents accidental evidence loss and makes every run independently auditable. The canonical session manifest is written before the event header, and its exact hash is bound through the event chain and completion manifest. Verify a finished recording with `termtrace verify /tmp/example.termtrace --level content --json`.

After content verification, optionally create a detached OpenSSH receipt without importing key material into Termtrace:

```bash
termtrace sign /tmp/example.termtrace --key ~/.ssh/evidence_ed25519 --signer evidence@example
termtrace verify /tmp/example.termtrace --level signature \
  --allowed-signers ./evidence-allowed-signers --json
```

The receipt signs canonical session/completion roots and the current artifact-root checkpoint. Verification reports cryptographic validity separately from whether the caller-supplied allowed-signers policy trusts the asserted identity. A locally stored receipt does not prevent deletion or complete replacement by an attacker controlling the signing key.

Detailed references: [installation](docs/installation.md), [agent workflows](docs/agent-workflows.md), [semantic replay](docs/semantic-replay.md), [formats and schemas](docs/formats.md), and [troubleshooting](docs/troubleshooting.md).

## Reproducible demos

Run `demos/run-all.sh` to generate three owner-only showcases covering visual/semantic fidelity, live agent control and signals, tmux interoperability, schema generation, tamper rejection, and torn-tail recovery. Each visual demo includes the canonical session, lossless 1080p/24 FPS Ghostty capture, screenshot, agent-readable states, asciicast export, and a broadly playable MP4. See [the demo guide](demos/README.md) for individual commands and artifact layout.

## Live agent control

Every active recording exposes an owner-only `control.sock` inside its session directory. Requests use bounded, newline-framed, versioned JSON and Linux peer credentials must match the recording owner. The CLI is a thin client:

Canonical paths remain evidence identity. For a memorable local handle, register an existing owner-only session and resolve it without rewriting evidence:

```bash
termtrace start demo --cols 120 --rows 40 -- your-command
termtrace restart demo
termtrace status demo
termtrace register demo --session /tmp/example.termtrace
termtrace resolve demo
termtrace resolve demo --json
termtrace list
termtrace list --all --json
termtrace remove demo
```

Registry entries live under an owner-only per-user state root, bind the immutable session ID and exact canonical path, and are created without replacement. Live registration also retains the recorder peer PID and Linux process-start identity so listing can detect stale sockets without PID-reuse mistakes. `list` shows live aliases by default; `--all` includes completed, incomplete, stale, missing, identity-mismatched, and incompatible entries with diagnostics. `remove` deletes only the alias file and never session evidence. `TERMTRACE_STATE_DIR` selects an explicit root; otherwise XDG state conventions are used.

`start NAME` accepts the same cwd, geometry, profile, durability, provenance, and command flags as `record`, chooses a collision-free owner-only path under the state root, launches `record --no-input`, and publishes the alias only after both the immutable manifest and control peer identity are valid. Mirrored output and recorder diagnostics go to owner-only supervisor logs outside canonical evidence. Startup failure reports those logs and the recoverable session candidate; it does not silently delete incomplete evidence.

`restart NAME` completes the current recording, clones its exact effective command, cwd, grid and pixel geometry, terminal colors/profile, durability policy, and provenance policy into a fresh canonical session, and atomically retargets the alias only after successor readiness. Command, cwd, grid/cell geometry, terminal identity, durability thresholds, and provenance policy can be overridden explicitly on the restart command; every omitted field stays inherited. Its JSON result contains both identities and paths. The new manifest binds the predecessor manifest and final event roots; predecessor evidence is never reopened for writing. A failed successor remains a reported recoverable orphan while the alias keeps its previous owner.

```bash
termtrace status /tmp/example.termtrace
termtrace send /tmp/example.termtrace \
  text:$'inspect src\n' --intent 'run repository inspection'
termtrace send /tmp/example.termtrace up
termtrace wait /tmp/example.termtrace 'Ready' --stable-for 250ms
termtrace wait /tmp/example.termtrace --output 'earlier diagnostic'
termtrace show /tmp/example.termtrace
termtrace show /tmp/example.termtrace --format json-compact
termtrace show --input trace.ansi --cols 100 --rows 30
termtrace show --cols 100 --rows 30 -- my-tui --demo
termtrace resize /tmp/example.termtrace --cols 120 --rows 36
termtrace mark /tmp/example.termtrace --label before-build
termtrace signal /tmp/example.termtrace --signal interrupt
termtrace stop /tmp/example.termtrace --wait
```

The live-command positional target accepts a registry name or any path containing `/`. Use `name:VALUE` and `path:VALUE` to resolve an otherwise ambiguous string explicitly. `send` accepts a sequence of named keys (including arrows, navigation/function keys, `shift-tab`, and `ctrl-a` through `ctrl-z`), `text:VALUE`, `paste:VALUE`, `raw:VALUE`, and `raw-file:PATH` atoms, or one `--stdin` burst. Semantic paste consults live bracketed-paste mode and rejects delimiter/control injection instead of silently rewriting it. `--pace 35ms` delays between atoms and splits text only at Unicode scalar boundaries. The JSON response reports the sequence and every atom's operation ID, exact emitted bytes, byte count, and canonical range. Named keys are encoded inside the actor from current terminal modes; canonical evidence retains both the symbolic key and exact bytes. Mutating commands accept `--if-state HASH`; the actor compares that guard with its latest presented-state hash at the operation ordering point and returns `stale_observation` with the current hash and source boundaries without emitting an intent when they differ. For multi-atom sends the guard applies independently to every atom. The Rust API exposes `control::send_request` and `control::send_request_response`; retain and resend the same request UUID after a transport failure. The session actor returns the exact persisted result and never applies that mutation twice, even after its bounded memory cache has evicted the request. Reusing a UUID for a different mutation or guard is rejected. Read-only requests are observations and are deliberately excluded from permanent deduplication.

The socket disappears when the child exits. `stop --wait` does not return until `completion.json` is durable and binds the session manifest, event header, final `Exit`, hash, count, output byte count, and exit outcome. `status --wait-complete` provides the same completion barrier without sending a signal. Marker responses include their canonical event sequence.

`show SESSION` prints the latest presentation-eligible viewport. A live session uses the actor snapshot fast path and is labeled `capture_quality: live`; a completed session validates completion and the event chain before replay and is labeled `completed`. `--format json` or `json-compact` emits the stable truth-labeled observation envelope. `--format vt|html` renders the exact selected completed state through the pinned Ghostty formatter; ANSI/stdin and one-off sources support the same formats, while live sessions currently require text/JSON. Completed evidence accepts `--state`, `--event-seq`, `--at`, or unique `--at-marker` selectors. Explicit selectors still reject synchronized-output hidden states unless `--include-hidden` is present; the default always chooses the latest presented state.

`show --input PATH` renders an ANSI/VT byte stream at explicit `--cols`/`--rows` geometry; `--input -` reads standard input. Structured output labels this source `ansi_stream`, `integrity: unchained`, and `capture_quality: incomplete`: it is a semantic convenience view, not canonical execution evidence.

`show -- COMMAND ...` runs a one-off command in a PTY, retains its owner-only canonical session, waits for durable completion, then renders the final presented viewport. Its structured observation is labeled `source_kind: one_off_command`, `integrity: canonical_chained`, and includes the retained session identity/path. Child output is kept in the recording rather than mixed into the observation response.

`timeline SESSION` is the concise discovery view for completed evidence. It merges presentation-eligible semantic transitions with canonical inputs, resizes, signals, markers, queries, lifecycle, errors, and exit. Use `--all-states` to include synchronized-output internals, `--controls` to interleave lossless printable/control frames with exact event byte ranges, `--jsonl` for one stable object per line, and the shared `--from`/`--to` or marker range selectors to bound replay.

`diff SESSION --from-state A --to-state B` compares two canonical semantic endpoints. Each endpoint also accepts `--from-event-seq`/`--to-event-seq`, `--from-at`/`--to-at`, or unique marker selectors. JSON contains complete before/after cells plus geometry, cursor, modes, colors, active style, metadata, presentation, hashes, and dirty evidence. `--expect-no-change` exits unsuccessfully when any normalized semantic value differs.

`explain SESSION --event-seq N` resolves the associated semantic state and reports exact framed bytes plus nearby semantic transitions. Targets also accept `--state`, `--at`, or `--marker`; `--context` selects the event radius. Stable findings currently identify full viewport clears, scrollback clears, hidden cell-changing transients, and clear/repaint candidates. Candidate facts explicitly avoid claiming native pixels flickered.

`output SESSION --raw` validates completed evidence and writes only the exact concatenation of canonical child PTY output bytes. It performs no UTF-8 decoding, control stripping, newline insertion, or input synthesis. Half-open `--from-event-seq`/`--to-event-seq` bounds and the shared time/marker range selectors restrict whole canonical output events. `--follow` uses actor-reported durable boundaries, validates each complete prefix, resumes by event sequence without duplication, and exits after validated completion; live following currently accepts event-sequence bounds only.

`events SESSION` streams stable JSONL projections of validated canonical records. `--type output,input_intent,marker` filters variants; event/time/marker bounds use the same half-open contract. `--metadata-only` removes exact `data`, `query`, and `response` bytes while retaining all non-byte metadata plus a BLAKE3 and byte count for each elided field. `--follow` validates successive actor-durable prefixes, preserves filters and metadata policy, resumes by event sequence without duplicates, and completes only after validating final evidence.

Generate completion scripts directly from the installed command graph with `termtrace completions bash|zsh|fish|elvish|powershell`. For example, Zsh users can write `termtrace completions zsh > ~/.zfunc/_termtrace` and ensure that directory is on `fpath` before `compinit`.

Run `termtrace doctor` for bounded local capability probes or `termtrace capabilities --json` for the stable inventory. `doctor --require core,semantic` fails unless every named tier is ready; `--explain` prints focused remediation for blocked tiers. Native screenshot and video readiness remain separate, and video checks FFmpeg's `libx264rgb` encoder. `termtrace version --json` reports package/build, semantic-engine, format/protocol, OS, and architecture identifiers.

`save SESSION --format txt,json,deep-json,vt,html --out NEW_DIRECTORY` resolves one presented semantic state and publishes all requested representations atomically. State/event/time/marker selectors match `show`; hidden states require explicit permission. VT and HTML come from the pinned formatter at that exact state, not the final-state convenience artifacts. `save-manifest.json` binds every member's path, media type, byte count, and BLAKE3 to one canonical session/final-event root and semantic state hash, and labels terminal screen content as potentially secret. Existing destinations are never replaced.

## Semantic replay

```bash
termtrace index --session /tmp/example.termtrace
```

Replay requires and validates `completion.json`, checks the event hash chain, feeds each output byte through the pinned libghostty-vt parser, and stores every distinct state under `derived/`. Synchronized-output intermediate states remain indexed with `presented: false`; video frame rate never limits semantic evidence. `index --live-snapshot` is the explicit exception for an active or interrupted recording: a responsive actor first reports a durable event-count boundary, Termtrace validates exactly that prefix while ignoring later concurrent appends, and writes a source-hash-addressed `derived-…-snapshot-HASH/` directory marked `source_complete: false`. Native capture will not consume it as final evidence.

- `states.jsonl.zst` is a Zstandard-compressed JSONL stream containing cursor, modes, colors, active SGR style, OSC metadata, Ghostty dirty rows, and cell-level grapheme/style/color diffs.
- `state-index.jsonl` maps state hashes to event sequence, byte offset, monotonic time, and presentation status.
- `final.txt`, `final.vt`, and `final.html` come from Ghostty's formatter.
- `manifest.json` pins the source event hash, schema version, Ghostty revision, final state hash, and whole-file length/BLAKE3/type/sensitivity metadata for every payload member.

Completed semantic and native derivations are also chained in `artifact-roots.jsonl`, separately from the permanently closed event log. `termtrace verify /tmp/example.termtrace --level content --json` verifies the local catalog, manifests, and every payload member; it explicitly leaves artifact authenticity false without a trusted detached checkpoint. Create the no-replace receipt after publishing final derivations: it authenticates only the artifact-root prefix named by its count and hash, so later derivations leave their artifact authenticity unestablished.

The derived directory is assembled privately and atomically renamed with no-replace semantics. Existing results are never overwritten implicitly.

For presentation-focused analysis, `termtrace index --presented-only` omits states hidden inside DEC synchronized output while retaining the canonical event log unchanged. It writes `derived-presented/`; pass the matching `--presented-only` flag to capture. The manifest distinguishes retained `state_count` from complete `semantic_state_count`.

Index only a half-open source interval when a long recording needs focused analysis:

```bash
termtrace index --session /tmp/example.termtrace \
  --from 00:08:15 --to 00:08:45

# Equivalent bounds using unique canonical markers:
termtrace index --session /tmp/example.termtrace \
  --from-marker investigation-start --to-marker investigation-end
```

`--from`/`--from-marker` are inclusive and `--to`/`--to-marker` are exclusive. A marker label must occur exactly once. Offsets accept bare seconds, `ns`/`us`/`ms`/`s`/`m`/`h` suffixes, `MM:SS.fraction`, or `HH:MM:SS.fraction`. An omitted bound selects the corresponding session endpoint. Termtrace fast-forwards all earlier output and successful resizes through libghostty-vt, emits a complete baseline at the start, and then indexes only states in the selected interval. Range artifacts use deterministic `derived-range-FROM_NS-TO_NS/` directories and coexist with `derived/`.

## Native Ghostty pixels

```bash
termtrace capture --session /tmp/example.termtrace --fps 24
```

Capture uses the viewport retained by `--viewport`; recordings without one retain the 1280x720 default. Supplying both `--width` and `--height` overrides the viewport, not the terminal text area. Before playback, a separate parent-side Ghostty probe records `geometry-preflight.json` and verifies the actual grid and text pixels. A mismatch reports requested viewport, actual grid, and recorded grid immediately. Playback also writes a structured failure handshake, so child geometry or replay errors are surfaced instead of degrading into a timeout.

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
