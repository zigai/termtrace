---
name: termtrace
description: Operate, test, diagnose, capture, or review terminal applications through Termtrace while retaining canonical evidence. Use for named terminal sessions, presented-screen waits, typed mode-aware input, TUI scenarios, semantic or native screenshots, timeline diagnosis, evidence bundles, and edited presentation videos.
---

# Termtrace

Use the shortest workflow that preserves enough evidence for the task. Keep canonical session paths in results even when using friendly names.

## Safe default loop

1. Check `termtrace doctor --json` when the workflow needs optional native capabilities.
2. Start a fresh named session with explicit geometry: `termtrace start NAME --cols 120 --rows 40 COMMAND...`.
3. Wait on the presented viewport with `termtrace wait NAME TEXT`. Use `termtrace wait-idle NAME` when the next step needs the exact state that satisfied stability; never emulate it with wait-then-show. Use `--output TEXT` only when the requirement is that the child emitted bytes at any time. Use structural predicates such as `--screen-regex`, `--row-contains`, `--region-contains`, `--cursor-at`, or `--mode` when text alone is ambiguous.
4. Send semantic input with `termtrace send NAME text:VALUE key:enter`. Retain caller-owned request IDs when using the driver or a client library.
5. Observe with `termtrace show NAME`; mark important boundaries before and after an interaction.
6. Diagnose unexpected transitions with `timeline`, `diff`, and `explain` before requesting native pixels.
7. Stop with completion waiting, then verify or bundle the immutable evidence.

Read [references/commands.md](references/commands.md) only when exact flags or command forms are needed.

## Preserve semantic distinctions

- Treat `screen` as the latest presentation-eligible viewport. Never substitute output history for a screen assertion.
- Treat `output` as exact canonical child PTY bytes. Do not call it application logs.
- Use named keys, text, and paste for intent-aware input. Use raw bytes only when exact encoding is the task.
- Keep live, stable, completed, interrupted-prefix, semantic-rendering, native-capture, and presentation-output evidence labeled distinctly.
- Never claim a nominal video-frame mapping proves which pixels were presented.
- Reject or explicitly opt into hidden synchronized-output states.
- Treat `stable` as the only complete `wait-idle` quality by default. Use `--allow-incomplete` only when typed `child_completed`, `output_closed`, or `deadline` evidence is acceptable.

## Handle mutations and failures

- Reuse the same request ID only for the exact same mutation after a safe retry indication.
- Stop when an outcome is uncertain. Inspect the request journal and canonical evidence before sending a replacement mutation.
- Use `--if-state HASH` for interactions that depend on the viewport you observed.
- Preserve failure directories and incomplete sessions. Do not delete evidence while diagnosing cleanup, timeout, or child failures.
- Do not upload canonical input, output, environment values, native video, or raw transcripts unless the user explicitly authorizes sensitive evidence transfer.

## Choose an artifact

- Use `screenshot` for deterministic semantic cell/style truth.
- Use native capture only after `doctor` reports the required environment ready; retain geometry probes and exact profile hashes.
- Keep native Ghostty video as the default visual source. Use `video ... --source semantic` only when the user explicitly wants deterministic generated video without native display dependencies; label it semantic rendering and never imply Ghostty pixel parity.
- Use `bundle --profile screen` for the least-sensitive handoff, `debug` for additional operational metadata, and `forensic` only with explicit authorization.
- Use `video plan` before `video export`; review resolved marker ranges, frame counts, speed, holds, and source hashes.
- Use `review --json` or ordinary diagnostic commands for automation. Open the interactive reviewer only for human navigation.

## Run repeatable tests

Prefer `termtrace test SCENARIO.termtrace.json` to sleeps or human timing. Every attempt must launch a fresh canonical session. On failure, report the step, canonical path, final observation, and atomic failure directory.

For test isolation, prefer `--clear-env --copy-env PATH --color-policy never` or a typed client environment object. Pass secret environment specifications through stdin or driver APIs, not command-line values. Use `--output-byte-budget` to terminate runaway output without truncating canonical evidence.

Use the framework-neutral fixtures under `fixtures/terminal/` when validating line input, alternate-screen behavior, or terminal-query responses.

## Refresh the command reference

Run `python3 scripts/generate-command-reference.py --binary PATH --out references/commands.md` from this skill directory after CLI changes. Commit the regenerated reference only after its example commands and `--help` output pass repository tests.
