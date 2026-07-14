# Agent workflows

## Start and discover

Use `termtrace start NAME -- COMMAND ...` for the short detached workflow. It returns only after the owner-only manifest and control identity are ready and includes the canonical path/session ID plus diagnostic log paths. `termtrace record --no-input --out SESSION -- COMMAND ...` remains the explicit foreground/evidence-path primitive. `termtrace ping NAME` and `termtrace status NAME` are readiness probes; do not infer readiness from arbitrary screen text.

Register a memorable local name with `termtrace register NAME --session SESSION`; `termtrace resolve NAME` returns the byte-exact canonical path and `--json` returns the bound session ID and protocol metadata. Registration is no-replace. Live commands accept one positional target: a registered name, a path containing `/`, or an explicit `name:`/`path:` reference when the spelling would be ambiguous.

Use `termtrace list` for live aliases and `termtrace list --all --json` when diagnosing completed, stale, missing, or incompatible bindings. `termtrace remove NAME` removes only navigation metadata and reports `evidence_preserved: true`; evidence deletion is intentionally not part of that command.

Use `termtrace show NAME` for the latest presented viewport. The live fast path does not replay the event log; after completion the same command validates and replays canonical evidence. Add `--format json-compact` when automation needs source identity, state hash, presentation status, boundary, capture quality, cursor/modes, and payload hash.

Use `termtrace show --input trace.ansi` or pipe bytes to `--input -` only for an immediate semantic rendering of an ANSI/VT stream. Choose `--cols` and `--rows` when the default 80×24 geometry is unsuitable. This source is explicitly unchained and incomplete; do not treat it as session evidence.

Use `termtrace show --cols 100 --rows 30 -- COMMAND ...` for a disposable invocation whose final viewport and execution evidence are both needed. The command runs synchronously to durable completion; its canonical session is retained under the owner-only state root and identified in structured output. This form never detaches the command.

Choose provenance deliberately for the workload. The default `minimal` policy avoids retaining locale/timezone values. Use `--provenance reproducible` when those values are needed to recreate behavior, or `--provenance custom --provenance-env NAME=hash` to retain equality evidence without the value. `not-captured` records an explicit omission; it is different from an unselected variable.

Before handing off a reproducible session, inspect `execution_provenance.executable`, `invoked_file`, and `runtime.recorder.binary`. A `captured` content identity can be compared to candidate files; `unresolved` or `unavailable` is an explicit reproducibility gap rather than permission to infer a match.

## Drive safely

Use `send SESSION text:VALUE enter` for UTF-8 plus semantic keys, and `raw:`, `raw-file:`, or `--stdin` for exact bytes. `--pace DURATION` schedules client-side gaps while the actor remains available for output and host-query handling; paced text uses Unicode scalar boundaries. Named keys are resolved by the session actor against live keyboard modes, not guessed by the client. Every mutation has a caller-owned UUID, durable intent, explicit result, byte count, and actor-side deduplication. Library clients should retain a `ControlRequest` and use `send_request` with the same UUID after a transport failure. Never create a new UUID to retry input whose outcome is unknown. Exact outcomes persist in `requests.jsonl`, so a retry remains safe after the actor's bounded memory cache evicts the request; read-only observations are not retained there.

Signals are allowlisted: interrupt, terminate, hangup, user1, and user2. `SIGKILL` is intentionally absent. `stop` is a graceful `SIGTERM` request. Markers are durable timeline annotations, and a successful `mark` response names the canonical event sequence.

Terminal-generated replies are marked `source: host`. A `query_response` event preserves the triggering control sequence and exact response before the corresponding input intent/result. Agents should not synthesize a second response.

## Wait and inspect

```bash
termtrace wait SESSION 'ready>' --stable-for 250ms --timeout-ms 30000
termtrace inspect --session SESSION --format text
termtrace inspect --session SESSION --state 42 --format json
termtrace inspect --session SESSION --event-seq 120 --format json
termtrace inspect --session SESSION --monotonic-ns 500000000 --format json
termtrace inspect --session SESSION --state 42 --row 16 --format json-compact
```

`wait` searches exact canonical output bytes, including matches spanning PTY reads. Ping, status, wait polling, and live-snapshot boundary discovery do not append events. Every live response distinguishes the latest actor-observed boundary from the file-synchronized durable boundary; forensic readers consume the durable prefix unless explicitly labeled volatile. It fails explicitly on timeout or child exit.

`inspect` replays from canonical events and returns a complete state, not a delta. JSON includes graphemes, styles, resolved colors, cursor, all known modes, OSC metadata, dirty evidence, and provenance. Text is a compact viewport view. `--row N` emits only one complete row plus bounded record provenance, and `json-compact` avoids pretty-print overhead. Output commands treat a downstream broken pipe as a normal early consumer exit.

Start rendering diagnosis with `termtrace timeline SESSION`. The default text view lists canonical operations and presented semantic transitions with their event/byte positions and changed row/cell counts. Add `--jsonl` for streaming automation, `--all-states` when synchronized-output internals matter, or `--controls` for exact lossless VT frames and neutral common-control descriptions. Timeline ranges use the same half-open time and unique-marker selectors as indexing and capture.

Use `termtrace diff SESSION --from-marker BEFORE --to-marker AFTER` after locating endpoints. Structured output preserves complete grapheme/style/color cell values and separates minimal semantic changes from each endpoint's Ghostty dirty evidence. Add `--expect-no-change` in tests; it still prints the reviewable diff before returning a failing status when a change exists.

Use `termtrace explain SESSION --event-seq N --context 8` to attribute a suspicious region to exact output frames and nearby semantic states. Findings have stable codes, conservative confidence, token IDs, and explicit facts. Treat `clear_then_repaint_candidate` as semantic evidence only; it deliberately states that native pixel flicker was not established.

Use `termtrace output SESSION --raw` only when a byte-exact child-output stream is required. It is output history, not the visible viewport; use `show` for the latter. The command validates completion and the event chain before emitting bytes, preserves invalid UTF-8 and controls, and treats a downstream closed pipe as normal completion. With `--follow`, only actor-durable complete records are emitted; the follower retries the completion/socket handoff and never emits a partial JSONL tail.

Use `termtrace events SESSION --type marker,input_intent,exit` for typed canonical history. Add `--metadata-only` before sharing diagnostic output when exact child/input/query bytes are unnecessary; the projection retains non-byte fields and byte equality evidence without the payload. Screen text and remaining metadata may still be sensitive. `--follow` keeps the same projection/filter contract while advancing only through integrity-validated actor-durable prefixes.

Use `termtrace save SESSION --format txt,json,deep-json,vt,html --out ARTIFACTS` to hand off several views of exactly one state. Publication is owner-only, atomic, and no-replace; the manifest binds every member to the same resolved state and completed source root. The `json` recipe is the compact observation envelope, `deep-json` retains the complete reconstructed grid and styles, and VT/HTML use the pinned formatter at the selected replay boundary.

## Complete and derive

Use an explicit completion barrier before post-processing:

```bash
termtrace wait SESSION --output READY
termtrace mark SESSION --label settled
termtrace stop SESSION --wait
termtrace index --session SESSION --from-marker settled
termtrace capture --session SESSION --from-marker settled
termtrace verify SESSION --level content --json
```

For authenticated handoff, verify content before signing, retain the trust policy separately, and check it at review time:

```bash
termtrace sign SESSION --key SSH_KEY --signer BUILD_IDENTITY
termtrace verify SESSION --level signature --allowed-signers ALLOWED_SIGNERS --json
```

Do not treat a successful cryptographic check without an allowed-signers policy as trusted identity. Create the no-replace receipt after publishing the final derivations. It checkpoints only the artifact-root prefix that existed when it was created; additional derivations remain locally verifiable but unauthenticated by that receipt.

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
