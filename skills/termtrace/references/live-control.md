# Live control

Use clean CLI commands or the Python SDK throughout an automated workflow:

```sh
termtrace record -d app --until-screen Ready -- ./command
termtrace send app text:/settings enter --until-idle 100ms
termtrace screen app --since STATE
```

Pass `--json` to receive structured JSON objects on stdout:

```sh
termtrace record -d app --json --until-screen Ready -- ./command
termtrace send app --json text:/settings enter --until-idle 100ms
termtrace screen app --json --since STATE
```

`--until-screen` observes the presented viewport. It is not a search of all
canonical child output. Use `wait --output` separately only when byte history is
the requirement.

`send` is the unified terminal input driver. It applies all input atoms under one guard and one request journal
entry, then evaluates the requested postcondition without a caller-visible race.
Supported input forms include:
- **Keys**: named keys like `enter`, `tab`, `escape`, `backspace`, arrow keys (`up`, `down`, `left`, `right`), function keys (`f1`..`f24`).
- **Modifiers**: `ctrl+f`, `ctrl-c`, `alt-enter`, `shift-tab`.
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
