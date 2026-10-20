# Live control

Use clean CLI commands or the Python SDK throughout an automated workflow:

```sh
termtrace record -d app --until-screen Ready -- ./command
termtrace send app text:/settings enter --until-screen Settings --timeout 10s
termtrace screen app --since STATE
```

`send` prints the action, the resulting state hash, and the presented screen, so
it needs no follow-up `screen` call. Keep that output; redirecting it to
`/dev/null` hides why a step failed.

Pass `--json` to receive structured JSON objects on stdout:

```sh
termtrace record -d app --json --until-screen Ready -- ./command
termtrace send app --json text:/settings enter --until-screen Settings --timeout 10s
termtrace screen app --json --since STATE
```

## Waiting

- Wait for the text you expect with `--until-screen` or `--until-screen-regex`.
  `--until-idle` waits for output or a screen change after the send, then for
  the screen to stop changing for its window (default 500ms). Input with no
  reaction within `--reaction-timeout` (default 2s) settles as
  `outcome=unchanged`; a slower app needs a longer reaction timeout. Echoed input
  counts as a reaction, and loading pauses can return a partial screen.
- Pass `--screen diff` to `send` to print only moved row blocks and changed rows
  since just before the send; it prints the full screen when most rows changed.
- Never `sleep` before reading the screen. When something outside Termtrace
  changes the app (tmux keys, a file edit, a reload), use
  `termtrace wait app --screen TEXT`.
- Waits default to 30s. Pass `--timeout` shorter than your shell tool's command
  timeout, so Termtrace reports the miss and the screen instead of being killed.

`--until-screen` observes the presented viewport. It is not a search of all
canonical child output. Use `wait --output` separately only when byte history is
the requirement.

`wait --screen` and `wait --idle` also answer for a session that has completed: the
predicate is evaluated once on the final presented screen (`quality: completed`), so a
program that prints and exits still satisfies a wait for its last output. A match counts
when it met the requested stability, or when it held on the final screen of a completed
child; pass `--require-live-stability` to `wait --idle` to reject completed sessions.

`send` is the unified terminal input driver. It applies all input atoms under one guard and one request journal
entry, then evaluates the requested postcondition without a caller-visible race.
Supported input forms include:
- **Keys**: named keys like `enter`, `tab`, `escape`, `backspace`, arrow keys (`up`, `down`, `left`, `right`), function keys (`f1`..`f24`).
- **Modifiers**: `ctrl+f`, `ctrl-c`, `alt-enter`, `shift-tab`, stacked as `ctrl+shift+x` or `super-f5`. The Python and Node `send_key`/`sendKey` accept the same grammar.
- **Pacing**: `--pace 20ms` types `text:` atoms one character at a time.
- **Text**: `text:any UTF-8 text`, `paste:pasted text`.
- **Mouse**: `click:COL,ROW` (e.g. `click:10,20`).
- **Activation**: `activate:"Button Text"` (finds visible row-contained text and activates it).
- **PTY Resize**: `resize:COLS,ROWS` atom or `--resize COLSxROWS` flag.
- **Signals**: `signal:SIGINT`, `signal:SIGTERM`, `signal:SIGHUP`.
- **Bookmarks**: `mark:LABEL` (records a named milestone marker in the event log).
- **Standard Input**: `--stdin` or `-` atom writes piped standard input bytes directly.

Pass the state returned by `record`, `send`, or `screen` through `--if-state` when
input is valid only for that presentation. Use the same state with
`screen --since STATE`; an unchanged response proves that no later presented
screen differs from it without retransmitting the viewport.

When `--json` is passed, `send` reports input application separately from its postcondition.
Treat `effect`, `retry`, and `session_state` as authoritative even when the
overall operation failed. `effect.kind` is `not_applied`, `partial` (with
`completed_atoms`), `applied`, or `unknown`; `retry` is derived from it. A failed
postcondition does not undo successfully applied input. Follow `retry` exactly:
`same_request_id` resends the identical request ID and payload, `new_request`
requires a corrected request, `inspect_first` means inspect the screen before
continuing past the applied prefix, and `do_not_retry` means the input already
applied. Never resend a whole sequence merely because its postcondition was unmet.
