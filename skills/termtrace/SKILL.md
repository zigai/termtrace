---
name: termtrace
description: Flight recorder for terminal applications through Termtrace. Use when work needs presented-screen synchronization, mode-aware input, canonical timeline diagnosis, repeatable TUI scenarios, screenshots, evidence bundles, or presentation videos.
disable-model-invocation: false
user-invocable: true
---

# Termtrace

Use the smallest evidence-producing workflow that answers the task. Termtrace's
normal CLI output is already compact; request full state, verbose atom outcomes,
or `--all` only when the next decision needs them.

## Choose the interface first

- Use one CLI command for one observation, wait, mutation, or artifact.
- Use `termtrace client python -` or `termtrace client typescript -` when the
  workflow has more than three actions, branching, repeated observations, or
  caller-owned retry IDs. These runners use installed SDK assets and managed
  runtime caches; they create no package, config, or generated source in the
  working tree. Read [client workflows](references/client-workflows.md).
- Use `termtrace test` for a repeatable assertion workflow or regression. Start
  from [the tested scenario example](examples/ready.termtrace.json) and read
  [repeatable testing](references/testing.md).
- Use the supplied canonical session directly for review. Never relaunch an
  application merely to inspect existing evidence.

## Choose the truth source

- Presented UI requirement: `wait SESSION --screen ...`, `wait-idle`, or `show`.
- Exact child byte-history requirement: `wait SESSION --output ...` or `output`.
- Semantic transition: `timeline`, `diff`, then `explain` as needed.
- Native pixels: request native screenshot/video explicitly; semantic images do
  not become native evidence.
- Handoff or publication: use a manifested save, screenshot, or bundle and read
  [artifact branches](references/artifacts.md).

Read [live control](references/live-control.md) before state-dependent input.
Read [the command reference](references/commands.md) only for the command family
being used; it is generated from the tested command graph.

## Live workflow

1. Start fresh evidence or resolve the supplied session.
2. Synchronize on the presented screen unless exact output history is the real
   requirement. Use `wait-idle` when the action must use the exact stable state it
   returns; do not split that into a wait followed by a second observation.
3. Guard state-dependent input with the returned presented-state hash. A hash
   emitted by `show` or `wait-idle` can be used directly with `--state-hash` for
   completed-state commands.
4. Mark meaningful workflow milestones only: phase entry, failure boundary, or
   a point requested by the user. Do not mark before and after every mutation;
   canonical input intents/results already identify those operations.
5. On a transport failure marked uncertain, retry only the identical payload
   with the identical request ID. On `input_sequence_incomplete`, use its exact
   attempted-prefix details; never resend the whole sequence with a new ID.
6. Stop with completion waiting, then verify or publish the required artifact.
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
[reference maintenance](references/maintenance.md).
