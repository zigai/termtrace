# Artifact branches

Choose the least-sensitive artifact that proves the requested outcome.

## Screenshots and native pixels

- Use `termtrace screenshot SESSION -o DIRECTORY` for manifested semantic PNG
  or SVG output (saving `screen.png` and manifest inside the output directory),
  exact selectors, or explicit native capture.
- Use native capture only after `termtrace doctor --json` reports the
  required environment ready. Retain the geometry probes and exact profile
  hashes.

## Presentation video

- Keep native Ghostty video as the default visual source.
- Use semantic video only when the user explicitly wants deterministic generated
  video without native display dependencies. Label it semantic rendering and
  never imply Ghostty pixel parity.
- Use `termtrace video SESSION -o FILE.mp4` for export,
  `--preview-out DIRECTORY` for review output, or `--dry-run` to inspect the resolved plan.
  Select an explicit preset, renderer, frame rate, resolution, marker range, or
  strict `--edit` plan when the requested deliverable requires it. Check every
  resolved marker range, frame count, speed, hold, and source hash before
  rendering.
- A video artifact is complete only when its source type, full render
  configuration, plan, and provenance are retained with the validated output.
- For a `record --original-native` session, render with
  `--renderer original-execution`; it uses only the captured pixels and fails when
  they are absent. GIF/WebM envelopes record `source_kind`. The MP4 sidecar records
  its source video path and hash; compare that hash with `visual-original/manifest.json`.

## Bundles and review

- Use `termtrace bundle SESSION -o DIRECTORY` for the least-sensitive manifested
  screen handoff (`--profile screen` by default).
- For a bounded debugging handoff, use `termtrace bundle SESSION --profile debug -o DIRECTORY`.
  This packages redacted metadata, the last screen, last-action analysis,
  diagnostics, and a canonical session reference without raw event streams.
  Add `--screenshot` to include a PNG screenshot of the last presented screen.
- Use `termtrace bundle SESSION --profile forensic -o DIRECTORY` for canonical sensitive evidence
  only when full byte-exact event streams are authorized. Verify with
  `termtrace integrity verify-bundle BUNDLE --source SESSION --profile PROFILE`.
- Use bare `termtrace analyze SESSION` for immediate triage of the last action and diagnostic findings.
  Open the interactive reviewer (`termtrace analyze review SESSION`) for visual human navigation.
- A handoff is complete only when its sensitivity profile is stated and its
  published artifact validates.
