# Artifact branches

Choose the least-sensitive artifact that proves the requested outcome.

## Screenshots and native pixels

- Use `termtrace screenshot SESSION -o DIRECTORY` for manifested semantic PNG
  or SVG output, exact selectors, screenshot batches, or explicit native
  capture.
- Use native capture only after `termtrace doctor --json` reports the
  required environment ready. Retain the geometry probes and exact profile
  hashes.

## Presentation video

- Keep native Ghostty video as the default visual source.
- Use semantic video only when the user explicitly wants deterministic generated
  video without native display dependencies. Label it semantic rendering and
  never imply Ghostty pixel parity.
- Use `termtrace video SESSION --out FILE.mp4` for export,
  `--preview-out DIRECTORY` for review output, or `--dry-run` to inspect the resolved plan.
  Select an explicit preset, renderer, frame rate, resolution, marker range, or
  strict `--edit` plan when the requested deliverable requires it. Check every
  resolved marker range, frame count, speed, hold, and source hash before
  rendering.
- A video artifact is complete only when its source type, full render
  configuration, plan, and provenance are retained with the validated output.

## Bundles and review

- Use `termtrace bundle SESSION -o DIRECTORY` for the least-sensitive manifested
  screen handoff.
- Use `termtrace bug-report SESSION -o DIRECTORY` for a bounded debugging
  handoff containing redacted metadata, the last screen, last-action analysis,
  diagnostics, and a canonical session reference. Add `--screenshot` only when
  image evidence is required. Raw input/output event streams are excluded.
- Use `termtrace bundle SESSION --profile debug` for redacted operational
  metadata or `--profile forensic` for canonical sensitive evidence only when
  that additional content is authorized. Verify with `termtrace integrity
  verify-bundle BUNDLE --source SESSION --profile PROFILE`.
- Use `termtrace inspect SESSION` and `termtrace diagnose SESSION` for ordinary
  automation.
  Open the interactive reviewer only for human navigation.
- A handoff is complete only when its sensitivity profile is stated and its
  published artifact validates.
