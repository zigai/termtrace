---
name: termtrace
description: Flight recorder for terminal applications through Termtrace. Use when work needs presented-screen synchronization, mode-aware input, canonical timeline diagnosis, repeatable TUI scenarios, screenshots, evidence bundles, or presentation videos.
disable-model-invocation: false
user-invocable: true
---
Use the smallest evidence-producing workflow that answers the task. Pass
`--response agent` for a single compact, versioned JSON response. Use
`--response full` only when debugging Termtrace itself requires forensic
internals. Never redirect a normal response to a temporary file and parse it
with `rg`, `sed`, `jq`, or similar shell filters; select the response profile,
screen encoding, or `--fields cursor,modes,boundary,timing` at the command
boundary instead.

## Choose the interface first

- Use `start --until-*` for initial synchronization, `act` for input plus its
  postcondition, and `screen --since STATE` for repeated observations. After
  completion, use `inspect SESSION` and `diagnose SESSION` before descending
  into advanced analysis selectors.
- Keep a persistent `termtrace agent` driver after two or three expected
  actions, or whenever the workflow needs branching, loops, repeated
  observations, multiple sessions, or caller-owned retry IDs. Read
  [client workflows](references/client-workflows.md).
- Use `termtrace test` for a repeatable assertion workflow or regression. Start
  from [the tested scenario example](examples/ready.termtrace.json) and read
  [repeatable testing](references/testing.md).
- Use the supplied canonical session directly for review. Never relaunch an
  application merely to inspect existing evidence.

## Choose the truth source

- Presented UI requirement: `screen`, `wait --screen`, or an `--until-screen`
  postcondition. The presented screen is the latest presentation-eligible
  viewport, not a transcript.
- Exact child byte-history requirement: `wait --output` or `analyze output`.
- Semantic transition: start with `inspect SESSION` or `diagnose SESSION`; use
  `analyze timeline`, `analyze diff`,
  and `analyze explain` only when explicit endpoints or forensic detail are
  needed.
- Native pixels: request native screenshot/video explicitly; semantic images do
  not become native evidence.
- Handoff or publication: use top-level `screenshot`, `bundle`, or `bug-report`
  and read [artifact branches](references/artifacts.md).

Read [live control](references/live-control.md) before state-dependent input.
The root help explains the ordinary workflow:

```text
termtrace --help
termtrace COMMAND --help
```

Consult only the direct command help needed for the task. Do not recursively
traverse the command tree or shell-filter help text. Never infer a path or flag
from an older command name.

## Live workflow

1. Start fresh evidence with `--until-screen` or `--until-idle`, or resolve the
   supplied session. This combines readiness with initial synchronization.
2. Use `act` to combine state-guarded input and the next screen or idle
   postcondition. Do not split it into `send` followed by a wait.
3. Use the returned presented-state hash with `screen --since STATE` to receive
   only a changed screen. Pass it through `--if-state` when an action is valid
   only for that exact presentation.
4. After two or three actions, keep one persistent `termtrace agent` process
   instead of repeatedly starting CLI processes.
5. Mark meaningful workflow milestones only: phase entry, failure boundary, or
   a point requested by the user. Canonical input intents and results already
   identify mutations.
6. Treat response fields `applied`, `retry`, and `session_state` as
   authoritative. Never infer mutation success from a postcondition failure.
   Retry only as directed: uncertain mutations commonly require the identical
   request ID and payload, while deterministic rejection requires a corrected
   request.
7. Stop with completion waiting. Inspect or diagnose the last action, then
   publish the smallest sufficient `screenshot`, `bundle`, or `bug-report`.
   Preserve failed and incomplete evidence.

## Evidence invariants

- `screen` is the latest presentation-eligible viewport; output history cannot
  satisfy a screen assertion.
- Hidden synchronized-output states require explicit opt-in.
- `stable` is complete idle evidence. Accept deadline, output-closed, or
  child-completed quality only when the task permits incomplete stability.
- Label live, stable, completed, interrupted-prefix, semantic-rendering,
  native-capture, and presentation-output evidence distinctly.
- A nominal video-frame mapping does not prove which pixels were presented.
- Raw terminal input/output and native captures may contain secrets. Do not
  transfer them outside the requested scope without explicit authorization.
- Return the canonical session path and any preserved failure artifact path.

After changing the CLI or skill examples, follow
[skill maintenance](references/maintenance.md).
