---
name: termtrace
description: Flight recorder for terminal applications through Termtrace. Use when work needs presented-screen synchronization, mode-aware input, canonical timeline diagnosis, repeatable TUI scenarios, screenshots, evidence bundles, or presentation videos.
disable-model-invocation: false
user-invocable: true
---
Use the smallest evidence-producing workflow that answers the task. By default,
every CLI command emits human-readable, clean text and tables. Pass `--json` to
any command to receive machine-readable, 2-space formatted JSON on stdout. When
quiet execution is required, pass `-q` / `--quiet` to suppress stdout completely
and communicate strictly via exit code.

## Choose the interface first

- For quick one-shot inspection and control, use the CLI:
  - `record -d <NAME> --until-*` for background recording and initial synchronization.
  - `screen <SESSION> [--since <STATE>]` to inspect the live presented terminal viewport.
  - `send <SESSION> [INPUT]... [--until-*]` for semantic input (keys, clicks, resize, signal) plus postcondition.
  - `wait <SESSION>` to synchronize on screen text, output bytes, or idle stability.
  - `status [<SESSION>]` to inspect session lifecycle or list registered sessions (`--gc` to prune).
  - `analyze <SESSION>` after completion to triage the last action and view diagnostic findings.
  - `screenshot`, `video`, or `bundle` to generate self-contained artifacts.
  - Pass `--json` when structured data is required.
- For complex, stateful workflows (loops, retries, branching, multi-step agent drivers),
  use the Rust SDK (`termtrace_sdk`) or the agent driver rather than
  chaining dozens of subshell CLI calls. Read [client workflows](references/client-workflows.md).
- Use `termtrace test` for repeatable assertion workflows or regressions. Start
  from [the tested scenario example](examples/ready.termtrace.json) and read
  [repeatable testing](references/testing.md).
- Use the supplied canonical session directly for review. Never relaunch an
  application merely to inspect existing evidence.

## Choose the truth source

- Presented UI requirement: `screen`, `wait --screen`, or an `--until-screen`
  postcondition. The presented screen is the latest presentation-eligible
  viewport, not a transcript.
- Exact child byte-history requirement: `wait --output` or `analyze output`.
- Semantic transition & forensics: start with bare `analyze <SESSION>` for triage; use
  `analyze timeline`, `analyze diff`, and `analyze explain` when explicit endpoints or forensic detail are
  needed.
- Cryptographic validity: `analyze verify <SESSION>`.
- Native pixels: request native screenshot/video explicitly; semantic images do
  not become native evidence.
- Handoff or publication: use top-level `screenshot`, `video`, or `bundle`
  and read [artifact branches](references/artifacts.md). For debugging bundles,
  use `bundle --profile debug` (with optional `--screenshot`).

Read [live control](references/live-control.md) before state-dependent input.
The root help explains the flat workflow:

```text
termtrace --help
termtrace COMMAND --help
```

Consult only the direct command help needed for the task. Do not recursively
traverse the command tree or shell-filter help text. Never infer a path or flag
from an older command name.

## Live workflow

1. Start fresh evidence with `termtrace record -d my-app --until-screen Ready -- ./cmd`, or resolve the
   supplied session. This combines readiness with initial synchronization.
2. Use `send` to combine state-guarded input and the next screen or idle
   postcondition. Natural input atoms include keys (`enter`, `ctrl+c`), text (`text:hello`),
   mouse clicks (`click:10,20`), semantic activation (`activate:"Save"`), window resizing
   (`resize:120,40`), and signals (`signal:INT`). Wait for expected text with
   `--until-screen` and a `--timeout` shorter than your shell tool's timeout,
   never with `sleep`. `send` prints the resulting state and screen.
3. Use the returned presented-state hash with `screen --since STATE` to receive
   only a changed screen. Pass it through `--if-state` when an action is valid
   only for that exact presentation.
4. For multi-action or interactive loops, use the Rust SDK or state-guarded send sequences.
5. Mark meaningful workflow milestones only: `send my-app mark:checkpoint`.
6. Check exit codes and `--json` error structures. Exit code 0 indicates success;
   non-zero indicates failure. Diagnostic logs go to stderr via `TERMTRACE_LOG`.
7. Stop with completion waiting (`stop my-app --wait`). Triage the last action with `analyze my-app`,
   then publish the smallest sufficient `screenshot`, `video`, or `bundle`.
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
