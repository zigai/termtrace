---
name: termtrace
description: Flight recorder for terminal applications through Termtrace. Use when work needs presented-screen synchronization or mode-aware input, canonical timeline diagnosis, repeatable TUI scenarios, screenshots, evidence bundles, or presentation videos.
disable-model-invocation: false
user-invocable: true
---

# Termtrace

Treat each session as a flight recorder: use the shortest path that proves the
requested outcome, preserve the evidence when anything fails, and return the
canonical session path even when operating through a friendly name.

## Process

1. **Set the evidence target.** Assign every requested outcome to `screen`,
   `output`, a semantic state, or a published artifact using the distinctions below.
   Read [the command contract](references/commands.md) when exact forms or flags
   are needed, [artifact branches](references/artifacts.md) when producing media
   or a bundle, [repeatable testing](references/testing.md) when running
   scenarios, and [reference maintenance](references/maintenance.md) after CLI
   changes. This step is complete when every outcome has a truth source and every
   applicable branch reference is loaded.
2. **Establish the session.** For new work, start a fresh named session with
   explicit geometry and capture its canonical path. For review, use the supplied
   canonical evidence without relaunching the application. This step is complete
   when the session is ready or the offline evidence has passed the required
   verification.
3. **Synchronize live work.** Wait on the presented viewport by default. Use
   atomic idle waiting when the next action depends on the exact stable state;
   never replace it with wait-then-show. Use output-history or structural
   predicates only when their evidence semantics match the requirement. This step
   is complete when no live synchronization is needed or the returned observation
   itself proves the predicate, with `stable` quality unless an incomplete quality
   was explicitly accepted.
4. **Bracket each mutation.** When interaction is required, observe the current
   state and mark important boundaries before it. Prefer named keys, text, or
   paste; use raw bytes only when exact encoding is the task. Guard state-dependent
   input with the observed state hash, and retain caller-owned request IDs in
   driver or client workflows. Mark the boundary after the resulting observation.
   This step is complete when no mutation is required or each mutation has one
   known outcome and both relevant states are identifiable.
5. **Explain surprises.** Inspect the timeline, semantic diff, and exact-state
   explanation before requesting native pixels. If a mutation outcome is
   uncertain, inspect its request journal and canonical evidence before retrying.
   This step is complete when no unexplained transition remains, or the remaining
   uncertainty is stated without another mutation.
6. **Complete the record.** Stop with completion waiting when the child is still
   live, then verify or bundle the immutable evidence required by the task. This
   step is complete when the session has a typed terminal outcome, requested
   artifacts are validated, and the result includes the canonical path plus any
   preserved failure directory.

## Evidence invariants

- `screen` is the latest presentation-eligible viewport; output history cannot satisfy a screen assertion.
- `output` is exact canonical child PTY bytes, not application logs.
- Hidden synchronized-output states require explicit opt-in.
- `stable` is the only complete idle-wait quality by default. Accept
  `child_completed`, `output_closed`, or `deadline` only through an explicit
  incomplete-evidence branch.
- Label live, stable, completed, interrupted-prefix, semantic-rendering,
  native-capture, and presentation-output evidence distinctly.
- A nominal video-frame mapping does not prove which pixels were presented.
- Retry a request ID only for the identical mutation after a safe-retry indication; otherwise stop.
- Preserve incomplete sessions and failure directories during cleanup, timeout,
  and child-failure diagnosis.
- Never upload canonical input, output, environment values, native video, or raw
  transcripts without explicit authorization for sensitive evidence transfer.
