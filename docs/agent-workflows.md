# Agent workflows

## Start and discover

Run `termtrace record --no-input --out SESSION -- COMMAND ...` in a supervised background process. The owner-only `SESSION/control.sock` appears when control is ready. `termtrace ping` and `termtrace status` are explicit readiness probes; do not infer readiness from arbitrary screen text.

## Drive safely

Use `send --text` for UTF-8 and `send --file` for exact bytes. Every mutation has a caller-owned UUID, durable intent, explicit result, byte count, and actor-side deduplication. Library clients should retain a `ControlRequest` and use `send_request` with the same UUID after a transport failure. Never create a new UUID to retry input whose outcome is unknown.

Signals are allowlisted: interrupt, terminate, hangup, user1, and user2. `SIGKILL` is intentionally absent. `stop` is a graceful `SIGTERM` request. Markers are durable timeline annotations.

Terminal-generated replies are marked `source: host`. A `query_response` event preserves the triggering control sequence and exact response before the corresponding input intent/result. Agents should not synthesize a second response.

## Wait and inspect

```bash
termtrace wait --session SESSION --text 'ready>' --timeout-ms 30000
termtrace inspect --session SESSION --format text
termtrace inspect --session SESSION --state 42 --format json
termtrace inspect --session SESSION --event-seq 120 --format json
termtrace inspect --session SESSION --monotonic-ns 500000000 --format json
```

`wait` searches exact canonical output bytes, including matches spanning PTY reads. Each poll creates a durable status boundary so buffered output is inspectable. It fails explicitly on timeout or child exit.

`inspect` replays from canonical events and returns a complete state, not a delta. JSON includes graphemes, styles, resolved colors, cursor, all known modes, OSC metadata, dirty evidence, and provenance. Text is a compact viewport view.

## Complete and derive

After `completion.json` exists, run `index`, then optionally `capture`. Indexing is deterministic and native capture is deliberately derived: neither can alter raw events. Existing artifact directories are never replaced.

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
