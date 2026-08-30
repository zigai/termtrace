# Live control and inspection

Every active recording exposes an owner-only control socket. Use it to observe
the terminal, act on a guarded state, resize the PTY, add markers, or stop the
child.

## Use the response protocol

Pass `--response agent` in automation. Agent mode writes exactly one compact,
versioned JSON object to stdout; logs and progress stay on stderr. Human mode is
concise prose, `full` exposes forensic internals, and `quiet` communicates only
through the exit status. `auto` selects human output on a terminal and agent
output otherwise. `TERMTRACE_RESPONSE=agent` sets the same default.

Do not redirect normal responses to temporary files and extract fields with
shell text filters. Select a response profile, screen encoding, or
`--fields cursor,modes,boundary,timing` at the Termtrace command boundary.

## Start and synchronize

`start` creates a recording under the per-user state directory and publishes its
name after readiness. Add a startup postcondition so the first action begins
from an observed state:

```sh
termtrace --response agent start demo --cols 120 --rows 36 --until-screen Ready -- your-command
termtrace --response agent start demo --until-idle 250ms -- your-command
```

Use `record --no-input --out SESSION` when the evidence path must be chosen
explicitly.

## Inspect the presented screen

`screen` observes the latest presentation-eligible viewport:

```sh
termtrace --response agent screen demo
termtrace --response agent screen demo --since STATE
```

Pass the `state` from one response to `--since`. If the presented viewport has
not changed, the response says it is unchanged instead of retransmitting it.
Choose `--screen exact|text|sparse|diff|none` to control the representation;
`diff` requires `--since`.

For completed sessions, `show` remains the point-selection interface:

```sh
termtrace show SESSION --state 42
termtrace show SESSION --state-hash HASH
termtrace show SESSION --event-seq 120
termtrace show SESSION --at-marker before-build
```

`analyze inspect` returns the complete reconstructed state, including cells,
styles, colors, cursor, modes, and source boundaries:

```sh
termtrace analyze inspect --session SESSION --state 42 --format json
termtrace analyze inspect --session SESSION --state 42 --row 16 --format json-compact
```

Use `analyze output SESSION --raw` only for original child-output byte history.
Output history is not the presented viewport and cannot satisfy a screen
requirement.

## Wait for state

Name the truth source explicitly:

```sh
termtrace --response agent wait demo --screen Ready --stable-for 250ms --timeout 30s
termtrace --response agent wait demo --screen-regex 'Ready|Done' --timeout 30s
termtrace --response agent wait demo --output 'earlier diagnostic' --timeout 30s
```

`--screen` and `--screen-regex` inspect the current presented viewport.
`--output` searches canonical output history. Other predicates test rows,
regions, cursor properties, terminal modes, and state changes. Waits are
observations and do not append events.

## Act on the observed state

`act` combines input with its postcondition in one actor operation:

```sh
termtrace --response agent act demo text:build enter --until-idle 250ms --if-state STATE
termtrace --response agent act demo enter --until-screen 'Build complete'
```

This avoids the race and ambiguity of `send` followed by a separate wait. It
also reports input application separately from postcondition success. Treat
`applied`, `retry`, and `session_state` as authoritative. A failed postcondition
does not undo applied input and is not permission to resend it.

Input atoms include text, paste, named keys, raw bytes, and files:
`text:build`, `paste:multiple-lines`, `enter`, `ctrl+f`, `raw:1b`, and
`raw-file:input.bin`. Run `termtrace control keys` for the complete grammar.
Use `--pace 35ms` only when the application requires paced input.

Every `act` mutation has a request ID and a durable outcome. Follow the returned
`retry` instruction exactly. An uncertain outcome may require the identical
request ID and payload; deterministic actor rejection requires a corrected
request. Never resend a whole sequence merely because its postcondition failed.

For direct unguarded bytes with no semantic interpretation or postcondition,
use `send SESSION --text TEXT`, `--file PATH`, or `--stdin`.

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

Start with the task-level last-action reports:

```bash
termtrace inspect SESSION
termtrace diagnose SESSION
```

These commands derive the complete latest input request from its canonical
intent/result pairs. Inspection returns changed rows, cursor and mode movement,
generated output, presentation eligibility, and bounded warnings. Diagnostics
are restricted to the same action range.

Use the advanced analysis group only when the task needs explicit selectors:

```bash
termtrace analyze timeline SESSION
termtrace analyze diff SESSION --from-marker before-build --to-marker after-build
termtrace analyze explain SESSION --event-seq 120 --context 8
termtrace analyze diagnose SESSION --check full-redraw,transient,dirty
```

The timeline combines canonical operations with semantic state changes. `diff`
compares complete cell values. `explain` associates VT tokens with nearby
states. Diagnostics are conservative findings, not guesses about pixels that
were never observed.

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
termtrace session register demo --session /tmp/example.termtrace
termtrace session resolve demo
termtrace session list
termtrace session list --all --json
termtrace session remove demo
```

Removing a name never removes evidence. A positional target containing `/` is treated as a path; other values are treated as names. Prefix ambiguous values with `path:` or `name:`.

Disposable replay checkpoints are compressed and automatically bounded per session:

```bash
termtrace analyze cache status SESSION
termtrace analyze cache prune SESSION
```

For managed recordings under the Termtrace state root, `termtrace session gc` reports old completed unregistered candidates without deleting them. Add `--apply` only after reviewing the dry-run report. Registered, live, incomplete, invalid, and newest retained recordings are preserved.

## Automate repeated tests

`termtrace test` runs versioned JSON scenarios in fresh sessions. Scenarios can send input, wait for screen state, add markers, compare snapshots, run temporal assertions, and retain failure evidence.

Scenario launches accept `environment` and `outputByteBudget`. Snapshot matching uses the same atomic stable-screen primitive; it no longer waits and then races a second snapshot. Environment values are redacted from copied failure scenarios.

```bash
termtrace test SCENARIO.json --jobs 4 --reruns 1
```

Snapshot files change only with `--update-snapshots`. Each rerun receives a new session; failed attempts are retained.

For more than two or three actions, keep one `termtrace agent` process and begin
the JSONL protocol with `hello`. Typed Python, TypeScript, and Rust clients wrap
the same screen, keyboard, pointer, event, completion, and evidence contracts.
Driver handles are process-local; canonical session IDs and paths remain stable
across driver restarts.

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
termtrace bundle SESSION --profile screen --out BUNDLE --dry-run
termtrace bundle SESSION --profile screen --out BUNDLE
termtrace integrity verify-bundle BUNDLE --source SESSION --profile screen
```

Use `screen` for a small state handoff, `debug` for redacted event metadata, and `forensic` only when raw input and output are explicitly required. Bundles are never uploaded automatically.
Verification requires the expected source and profile so an untrusted manifest
cannot choose its own provenance or sensitivity policy.
