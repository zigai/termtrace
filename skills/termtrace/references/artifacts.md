# Artifact branches

Choose the least-sensitive artifact that proves the requested outcome.

## Screenshots and native pixels

- Use `screenshot` for deterministic semantic cell and style truth.
- Use native capture only after `doctor --json` reports the required environment
  ready. Retain the geometry probes and exact profile hashes.

## Presentation video

- Keep native Ghostty video as the default visual source.
- Use semantic video only when the user explicitly wants deterministic generated
  video without native display dependencies. Label it semantic rendering and
  never imply Ghostty pixel parity.
- Run `video plan` before `video export`. Check every resolved marker range, frame
  count, speed, hold, and source hash before rendering.
- A video artifact is complete only when its source type, plan, and provenance are
  retained with the validated output.

## Bundles and review

- Use the `screen` bundle profile for the least-sensitive handoff and `debug` for
  additional operational metadata. Use `forensic` only with explicit
  authorization.
- Use `review --json` or ordinary diagnostic commands for automation. Open the
  interactive reviewer only for human navigation.
- A handoff is complete only when its sensitivity profile is stated and its
  published artifact validates.
