---
name: termtrace
description: Drive and record terminal apps with the termtrace MCP tools, or the termtrace CLI when the tools are unavailable. Use when work needs presented-screen synchronization, mode-aware input, timeline diagnosis, repeatable TUI scenarios, screenshots, evidence bundles, or videos.
disable-model-invocation: false
user-invocable: true
---

# Termtrace

Use the smallest workflow that answers the task and produces evidence.

## Choose the interface

- When the termtrace MCP tools are available, use them for everything they
  cover: `start`, `send`, `wait`, `screen`, `status`, `stop`, `restart`, `gc`,
  `analyze`, `screenshot`, and `bundle`. Do not switch to the CLI for these.
- Read [the CLI reference](references/cli.md) only when the MCP tools are not
  available, or for CLI-only work: `video`, `test`, `tmux`, and `doctor`.
- For loops, retries, branching, or many sessions in code, use an SDK. Read
  [client workflows](references/client-workflows.md).
- For repeatable assertions, use `termtrace test` scenarios. Read
  [repeatable testing](references/testing.md).
- Review a supplied session directly. Never relaunch an app only to inspect
  evidence that already exists.

MCP sessions and CLI sessions share names, so either interface can pick up a
session the other started.

## Live workflow

1. `start` the program with an `until` condition, such as
   `{"kind": "screen", "text": "Ready"}`. This waits for readiness in the same call.
2. `send` inputs with the next `until` condition. The result carries the new
   state and screen, so it needs no follow-up `screen` call.
3. Wait for the text you expect. Never sleep before reading the screen. Use
   `wait` when something outside termtrace changes the app.
4. Keep `timeout_ms` below the MCP client's tool timeout, so termtrace reports
   the miss and the screen instead of being cut off.
5. Pass a returned `state` as `if_state` when input is valid only for that exact
   screen, or as `since` to `screen` to get only what changed.
6. Mark meaningful milestones only: `mark:checkpoint` as an input.
7. `stop` the session with `wait`, triage it with `analyze`, then produce the
   smallest sufficient `screenshot` or `bundle`. Keep failed and incomplete
   evidence.

To drive several sessions at once, `send` to each without `until`, then call
`wait` on each session in parallel. MCP clients may serialize mutating sends.

A session name is free once its session ends: `start` a new session under it,
or `restart` the completed one.

## Waiting

- `idle` waits for the screen to react to the input, then to stop changing for
  its interval (default 500 ms). Input with no reaction within
  `reaction_timeout_ms` (default 2000) settles as `unchanged`. Loading pauses
  can return a partial screen, so prefer `screen` text when you know it.
- A deadline result includes the screen and `changing_rows`, the rows that kept
  changing, such as a spinner or clock. Use them to pick a better condition.
- `settle_ms` requires the matched screen to stay unchanged before returning.
- `wait` on a completed session checks the final screen once and reports
  `outcome: completed`.
- The `wait:TEXT` input pauses a sequence until the screen shows `TEXT`.

## Send results

`send` reports the input separately from its condition. Treat `effect`,
`retry`, and `session_state` as authoritative even when the call failed.
A failed condition does not undo applied input. Follow `retry` exactly:

- `same_request_id`: resend the identical request with the same request ID.
  MCP `send` cannot pass a request ID, so read the screen before resending.
- `new_request`: send a corrected request.
- `inspect_first`: read the screen before continuing past the applied prefix.
- `do_not_retry`: the input already applied.

Never resend a whole sequence only because its condition was unmet.

## Choose the truth source

- Presented UI: `screen`, or an `until` of kind `screen`, `screen_absent`, or
  `screen_regex`. The presented screen is the latest viewport shown, not a
  transcript.
- Exact byte history: an `until` of kind `output`, or `analyze` with kind `output`.
- Transitions and forensics: `analyze` with kind `triage` first, then
  `timeline`, `diff`, or `explain` with explicit state hashes.
- Recording integrity: `analyze` with kind `verify`.
- Native pixels: a `native` screenshot. A live session's screenshot is native
  by default. Semantic images never count as native evidence.
- Handoff: `screenshot` or `bundle`, and read [artifacts](references/artifacts.md).
  Use the `debug` bundle profile for debugging.

## Evidence invariants

- `screen` is the latest presented viewport; output history cannot satisfy a
  screen assertion.
- Hidden synchronized-output states require explicit opt-in.
- `stable` is complete idle evidence. Accept deadline, output-closed, or
  child-completed quality only when the task permits incomplete stability.
- Label live, stable, completed, interrupted-prefix, semantic-rendering,
  native-capture, and presentation-output evidence distinctly.
- A nominal video-frame mapping does not prove which pixels were presented.
- Raw terminal input and output and native captures may contain secrets. Do not
  transfer them outside the requested scope without explicit authorization.
- Return the canonical session path and any preserved failure artifact path.

After changing the MCP tools, the CLI, or skill examples, follow
[skill maintenance](references/maintenance.md).
