# Live control and inspection

Every active recording exposes an owner-only control socket. Use it to observe the terminal, send input, resize the PTY, add markers, or stop the child.

## Start a detached session

`start` creates a recording under the per-user state directory and publishes its name only after the control socket is ready:

```bash
termtrace start demo --cols 120 --rows 36 your-command
termtrace control ping demo
termtrace status demo
```

Use `record --no-input --out SESSION` when the evidence path must be chosen explicitly.

## Inspect the screen

`show` is the simplest view of the latest presented terminal state:

```bash
termtrace show demo
termtrace show demo --format json-compact
```

For completed sessions, select a specific point:

```bash
termtrace show SESSION --state 42
termtrace show SESSION --state-hash HASH
termtrace show SESSION --event-seq 120
termtrace show SESSION --at-marker before-build
```

`inspect` returns the complete reconstructed state, including cells, styles, colors, cursor, modes, and source boundaries:

```bash
termtrace analyze inspect --session SESSION --state 42 --format json
termtrace analyze inspect --session SESSION --state 42 --row 16 --format json-compact
```

Use `analyze output SESSION --raw` only for the original child-output byte stream. Output history is not the same thing as the visible viewport.

## Wait for state

Wait for presented screen text:

```bash
termtrace wait demo 'Ready' --stable-for 250ms --timeout-ms 30000
```

Use `--output TEXT` when you need historical output rather than the current screen. Other predicates can test rows, regions, cursor properties, terminal modes, and state changes. See `termtrace wait --help` for the full predicate grammar.

Waits are observations. They do not append events to the recording.

When the next action needs the exact state that became idle, do not issue a wait followed by `show`; the state can change between calls. Use the actor-atomic operation:

```sh
termtrace wait-idle demo --stable-for 250ms --timeout-ms 30000
```

The default response contains the viewport text, state hash, geometry, cursor, modes, and one compact canonical boundary from the exact actor state. It deliberately omits the large compressed semantic-state blob; add `--full-state` only when a consumer will decode the complete cell model. The default accepts only `stable`. `--allow-incomplete` returns typed `child_completed`, `output_closed`, or `deadline` snapshots for explicit inspection.

## Send input

`send` accepts text, paste, named keys, raw bytes, and files:

```bash
termtrace send demo text:'build' enter
termtrace send demo up up enter
termtrace send demo paste:'multiple lines'
termtrace send demo raw-file:input.bin
termtrace send demo ctrl+f q enter
```

The complete atom list is admitted under one optimistic guard and one request ID. Use `--pace 35ms` to delay between atoms. Named keys are encoded from the application's live terminal modes, common `ctrl`/`shift`/`alt`/`super` combinations accept `+` or `-`, and a single printable character is text. `termtrace control keys` prints the accepted grammar. Paste input respects bracketed-paste mode and rejects unsafe delimiter injection. Add `--verbose` only when per-atom event coordinates are needed.

Every mutation has a request ID and a durable outcome. A transport failure labeled `safe_same_request_id` must be retried with the identical ID and payload. Deterministic actor rejections require a corrected request. An `input_sequence_incomplete` response reports the exact applied prefix, failed atom, partial byte count, and first unattempted suffix; never resend the whole sequence under a new ID. Library clients preserve these structured details.

## Guard a mutation

Structured observations include a presented `state_hash`. Pass it back with `--if-state` when an action is valid only for that state:

```bash
termtrace send demo enter --if-state HASH
```

If the screen changed first, Termtrace returns `stale_observation` without sending input. Inspect again before deciding what to do.

## Focus and pointer input

Focus reports work only when the application enabled focus reporting:

```bash
termtrace control focus demo in --strict
```

Pointer operations use zero-based terminal cells and the application's live mouse mode:

```bash
termtrace control pointer demo click --cell 24,8 --button left
termtrace control pointer demo drag --from 10,5 --to 40,5 --button left
termtrace control pointer demo scroll --cell 70,20 --lines -3
```

To click visible text, let Termtrace resolve the text and geometry from one state:

```bash
termtrace control activate demo --text 'Save changes' --unique --strict
```

These commands prove which bytes were emitted. Add a screen wait to prove how the application responded.

## Mark, signal, and stop

Markers create durable timeline anchors:

```bash
termtrace control mark demo --label before-build
termtrace control signal demo --signal interrupt
termtrace stop demo --wait
```

Allowed signals are `interrupt`, `terminate`, `hangup`, `user1`, and `user2`. `stop` sends a graceful termination request. `--wait` returns only after `completion.json` is durable.

## Review changes

Start with a timeline, then narrow the interesting range:

```bash
termtrace analyze timeline SESSION
termtrace analyze diff SESSION --from-marker before-build --to-marker after-build
termtrace analyze explain SESSION --event-seq 120 --context 8
termtrace analyze diagnose SESSION --check full-redraw,flicker,stale-cells
```

The timeline combines canonical operations with semantic state changes. `diff` compares complete cell values. `explain` associates VT tokens with nearby states. Diagnostics are conservative findings, not guesses about pixels that were never observed.

Potentially large report commands return at most 200 matching records by default. Continue deterministically with `--offset NEXT --limit N`, or request intentionally large output with `--all`. JSONL commands append a typed `output_page` record only when the result is partial.

Completed-state selectors also accept content hashes:

```bash
termtrace analyze diff SESSION --from-state-hash HASH1 --to-state-hash HASH2
termtrace analyze explain SESSION --state-hash HASH2
```

State hashes select exact semantic content; if identical content occurred more than once, Termtrace deterministically resolves its latest occurrence.

## Friendly names

Names are navigation metadata, not evidence identity:

```bash
termtrace registry register demo --session /tmp/example.termtrace
termtrace registry resolve demo
termtrace registry list
termtrace registry list --all --json
termtrace registry remove demo
```

Removing a name never removes evidence. A positional target containing `/` is treated as a path; other values are treated as names. Prefix ambiguous values with `path:` or `name:`.

Disposable replay checkpoints are compressed and automatically bounded per session:

```bash
termtrace analyze cache status SESSION
termtrace analyze cache prune SESSION
```

For managed recordings under the Termtrace state root, `termtrace registry gc` reports old completed unregistered candidates without deleting them. Add `--apply` only after reviewing the dry-run report. Registered, live, incomplete, invalid, and newest retained recordings are preserved.

## Automate repeated tests

`termtrace test` runs versioned JSON scenarios in fresh sessions. Scenarios can send input, wait for screen state, add markers, compare snapshots, run temporal assertions, and retain failure evidence.

Scenario launches accept `environment` and `outputByteBudget`. Snapshot matching uses the same atomic stable-screen primitive; it no longer waits and then races a second snapshot. Environment values are redacted from copied failure scenarios.

```bash
termtrace test SCENARIO.json --jobs 4 --reruns 1
```

Snapshot files change only with `--update-snapshots`. Each rerun receives a new session; failed attempts are retained.

For language integrations, keep one `termtrace tooling driver` process and begin with `hello`. Typed Python, TypeScript, and Rust clients wrap the same screen, keyboard, pointer, event, completion, and evidence contracts. Driver handles are process-local; canonical session IDs and paths remain stable across driver restarts.

For branching, retries, loops, or multi-session orchestration, run a reviewable
client script without generating a project:

```bash
termtrace tooling client status
termtrace tooling client python workflow.py -- --workflow-argument
termtrace tooling client typescript workflow.ts -- --workflow-argument
```

The managed runners use private temporary import mappings and runtime caches. They do not create package manifests, lockfiles, virtual environments, generated source, or `node_modules` in the caller's working directory.
Use standard input only for short disposable snippets; use `termtrace test` for
repeatable assertions.

## Hand off evidence

Preview the files and sensitivity labels before creating a bundle:

```bash
termtrace artifact bundle create SESSION --profile screen --out BUNDLE --dry-run
termtrace artifact bundle create SESSION --profile screen --out BUNDLE
termtrace artifact bundle verify BUNDLE --source SESSION --profile screen
```

Use `screen` for a small state handoff, `debug` for redacted event metadata, and `forensic` only when raw input and output are explicitly required. Bundles are never uploaded automatically.
Verification requires the expected source and profile so an untrusted manifest
cannot choose its own provenance or sensitivity policy.
