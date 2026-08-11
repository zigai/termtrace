# Artifact branches

Choose the least-sensitive artifact that proves the requested outcome.

## Screenshots and native pixels

- Use `termtrace screenshot SESSION -o FILE` for the ordinary deterministic
  semantic screenshot workflow.
- Use `termtrace artifact screenshot` only for exact selectors, screenshot
  batches, manifested multi-file output, or native capture.
- Use native capture only after `termtrace tooling doctor --json` reports the
  required environment ready. Retain the geometry probes and exact profile
  hashes.

## Presentation video

- Keep native Ghostty video as the default visual source.
- Use semantic video only when the user explicitly wants deterministic generated
  video without native display dependencies. Label it semantic rendering and
  never imply Ghostty pixel parity.
- Use `termtrace video SESSION --out FILE` for the normal export workflow. Select
  an explicit preset, renderer, frame rate, resolution, or marker range when the
  requested deliverable requires it.
- Use the deeper `artifact video plan` and `artifact video export` commands only
  when reviewing or editing the complete render plan. Check every resolved
  marker range, frame count, speed, hold, and source hash before rendering.
- A video artifact is complete only when its source type, full render
  configuration, plan, and provenance are retained with the validated output.

## Bundles and review

- Use `termtrace bundle SESSION -o DIRECTORY` for the least-sensitive manifested
  screen handoff.
- Use `termtrace bug-report SESSION -o DIRECTORY` for a bounded debugging
  handoff containing redacted metadata, the last screen, last-action analysis,
  diagnostics, and a canonical session reference. Add `--screenshot` only when
  image evidence is required. Raw input/output event streams are excluded.
- Use advanced `artifact bundle` profiles only when additional content is
  required: `debug` adds operational metadata and `forensic` includes canonical
  sensitive evidence and therefore requires explicit authorization.
- Use `termtrace inspect SESSION --last-action` and
  `termtrace diagnose SESSION --since-last-action` for ordinary automation.
  Open the interactive reviewer only for human navigation.
- A handoff is complete only when its sensitivity profile is stated and its
  published artifact validates.
