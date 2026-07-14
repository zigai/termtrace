# Agent workflows

## Start and discover

Run `termtrace record --no-input --out SESSION -- COMMAND ...` in a supervised background process. The owner-only `SESSION/control.sock` appears when control is ready. `termtrace ping` and `termtrace status` are explicit readiness probes; do not infer readiness from arbitrary screen text.

## Drive safely

Use `send --text` for UTF-8 and `send --file` for exact bytes. Every mutation has a caller-owned UUID, durable intent, explicit result, byte count, and actor-side deduplication. Library clients should retain a `ControlRequest` and use `send_request` with the same UUID after a transport failure. Never create a new UUID to retry input whose outcome is unknown.

Signals are allowlisted: interrupt, terminate, hangup, user1, and user2. `SIGKILL` is intentionally absent. `stop` is a graceful `SIGTERM` request. Markers are durable timeline annotations, and a successful `mark` response names the canonical event sequence.

Terminal-generated replies are marked `source: host`. A `query_response` event preserves the triggering control sequence and exact response before the corresponding input intent/result. Agents should not synthesize a second response.

## Wait and inspect

```bash
termtrace wait --session SESSION --text 'ready>' --timeout-ms 30000
termtrace inspect --session SESSION --format text
termtrace inspect --session SESSION --state 42 --format json
termtrace inspect --session SESSION --event-seq 120 --format json
termtrace inspect --session SESSION --monotonic-ns 500000000 --format json
termtrace inspect --session SESSION --state 42 --row 16 --format json-compact
```

`wait` searches exact canonical output bytes, including matches spanning PTY reads. Ping, status, wait polling, and live-snapshot boundary discovery do not append events. Every live response distinguishes the latest actor-observed boundary from the file-synchronized durable boundary; forensic readers consume the durable prefix unless explicitly labeled volatile. It fails explicitly on timeout or child exit.

`inspect` replays from canonical events and returns a complete state, not a delta. JSON includes graphemes, styles, resolved colors, cursor, all known modes, OSC metadata, dirty evidence, and provenance. Text is a compact viewport view. `--row N` emits only one complete row plus bounded record provenance, and `json-compact` avoids pretty-print overhead. Output commands treat a downstream broken pipe as a normal early consumer exit.

## Complete and derive

Use an explicit completion barrier before post-processing:

```bash
termtrace wait --session SESSION --text READY
termtrace mark --session SESSION --label settled
termtrace stop --session SESSION --wait
termtrace index --session SESSION --from-marker settled
termtrace capture --session SESSION --from-marker settled
termtrace verify SESSION --level content --json
```

Normal `index` and every `capture` require `completion.json` to bind the exact session manifest and event header, agree with the canonical final hash, event count, output byte count, and exit outcome, and identify a final `Exit` with no pending mutation. This prevents a settled-but-live TUI from producing an index that silently becomes stale. `status --wait-complete` waits without stopping the child. If a live prefix is genuinely needed, `index --live-snapshot` writes a hash-addressed, explicitly incomplete directory; create a fresh final index after completion. Existing artifact directories are never replaced.

Use `index --presented-only` (and the same capture flag) when synchronized-output internals are unnecessary. Full indexing remains the default. Marker labels used as `--from-marker` or `--to-marker` must occur exactly once.

## tmux

tmux is an ordinary target, not a source of truth:

```bash
termtrace record --out SESSION -- tmux new-session -A -s agent
```

PTY bytes, input, resize, and signals remain canonical. tmux capture-pane may be useful for comparison, but it loses transient states and is not used for replay.

## asciinema

```bash
termtrace export-asciicast --session SESSION --out session.cast
```

Asciicast v2 is UTF-8-only. Invalid bytes become replacement characters, timing becomes floating-point seconds, and advanced semantic state is absent. The cast is interoperability output, never canonical evidence.
