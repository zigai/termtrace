# CLI

Use the CLI when the termtrace MCP tools are not available, or for CLI-only
commands: `video`, `test`, `tmux`, and `doctor`. The live workflow, waiting
rules, send results, and truth sources in the skill apply here too, with the
flags below in place of MCP parameters.

## Output

Every command prints clean text and tables by default. Pass `--json` for
2-space formatted JSON on stdout. Pass `-q` / `--quiet` to suppress stdout and
report only through the exit code. Exit code 0 means success. Diagnostic logs go
to stderr through `TERMTRACE_LOG`.

## Commands

- `record -d <NAME> --until-* -- COMMAND` records in the background and waits
  for readiness.
- `send <SESSION> [INPUT]... [--until-*]` applies input and waits for a condition.
- `screen <SESSION> [--since <STATE>]` reads the presented screen.
- `wait <SESSION>` waits for screen text, output bytes, or idle stability.
- `status [<SESSION>]` shows one session or lists them; `--gc` prunes old ones.
- `stop <SESSION> --wait` stops a session and waits for completion.
- `restart <SESSION>` relaunches a completed session under the same name.
- `analyze <SESSION>` triages the last action; `analyze timeline`, `diff`,
  `explain`, `output`, and `verify` give detail.
- `screenshot`, `video`, and `bundle` produce artifacts.

Read the root help, then only the direct command help the task needs:

```text
termtrace --help
termtrace COMMAND --help
```

Do not recursively traverse the command tree or filter help text through the
shell. Never infer a path or flag from an older command name.

## Live control

```sh
termtrace record -d app --until-screen Ready -- ./command
termtrace send app text:/settings enter --until-screen Settings --timeout 10s
termtrace screen app --since STATE
termtrace stop app --wait
```

`send` prints the action, the resulting state hash, and the presented screen, so
it needs no follow-up `screen` call. Keep that output; redirecting it to
`/dev/null` hides why a step failed.

## Conditions

The CLI spells conditions as `--until-screen`, `--until-screen-absent`,
`--until-screen-regex`, `--until-output`, and `--until-idle[=500ms]` on `record`
and `send`. `wait` takes `--screen`, `--screen-absent`, `--screen-regex`,
`--output`, or `--idle`.

- `--reaction-timeout` (default 2s) bounds how long `--until-idle` waits for a
  reaction before settling as `outcome=unchanged`.
- `--settle 200ms` requires a stable screen after a match.
- `send --screen` takes `text` (default), `state`, or `diff`. `diff` prints only
  moved row blocks and changed rows since just before the send.
- `wait --report` takes `text` (default), `state`, or `frame`.
- `wait --idle --require-live-stability` rejects completed sessions.
- Waits default to 30s. Durations require units, such as `1500ms` or `20s`.
  Pass `--timeout` shorter than your shell tool's timeout.
- `--if-state STATE` applies input only for that exact presentation.

`--until-screen` observes the presented viewport. It is not a search of all
output. Use `wait --output` only when byte history is the requirement.

## Inputs

`send` applies all input atoms under one guard and one journal entry, then
evaluates the condition without a caller-visible race. MCP `send` takes the same
atoms in `inputs`.

- **Keys**: `enter`, `tab`, `escape`, `backspace`, arrows (`up`, `down`, `left`,
  `right`), and `f1`..`f24`.
- **Modifiers**: `ctrl+f`, `ctrl-c`, `alt-enter`, `shift-tab`, stacked as
  `ctrl+shift+x` or `super-f5`. The Python and Node `send_key`/`sendKey` accept
  the same grammar.
- **Pacing**: `--pace 20ms` types `text:` atoms one character at a time.
- **Delay**: `text:hello sleep:200ms enter` pauses before Enter, so an app such
  as Codex submits typed text instead of treating Enter as part of a paste.
- **Synchronization**: `text:hello wait:Ready` pauses until the screen shows `Ready`.
- **Text**: `text:any UTF-8 text`, `paste:pasted text`.
- **Mouse**: `click:COL,ROW`, `hover:COL,ROW`, `scroll:COL,ROW,LINES` (negative
  lines scroll up), and `drag:COL,ROW,COL,ROW`; coordinates are zero-based.
- **Activation**: `activate:"Button Text"` finds visible text and activates it.
- **Resize**: `resize:COLS,ROWS` atom or `--resize COLSxROWS` flag.
- **Signals**: `signal:SIGINT`, `signal:SIGTERM`, `signal:SIGHUP`.
- **Bookmarks**: `mark:LABEL` records a named milestone.
- **Standard input**: `--stdin` or the `-` atom writes piped bytes directly
  (CLI only).

With `--json`, `send` reports `effect`, `retry`, and `session_state` as
described in the skill. `same_request_id` means resend with the same
`--request-id` and payload.
