# Semantic replay

Semantic replay turns canonical PTY output into inspectable terminal states.

```bash
termtrace analyze index --session SESSION
termtrace analyze inspect --session SESSION --format text
```

Replay validates the event chain and uses the exact pinned semantic engine. The same recording therefore produces byte-for-byte deterministic state artifacts when the engine and schema versions are unchanged.

Imported replay is bounded to 256 MiB of output, 2,000,000 capture attempts,
100,000 distinct states, 2,000,000,000 cell-work units, 512 MiB of emitted
artifact data, and 60 seconds. Geometry is independently capped at 1,000
columns, 1,000 rows, and 250,000 cells. Crossing any ceiling returns an error
and prevents artifact publication.

The pinned Ghostty submodule is a read-only dependency. Termtrace applies its
own embedder policy before semantic replay crosses the FFI boundary: OSC and
APC strings are capped at 1 MiB, and Kitty graphics storage is disabled because
semantic artifacts do not represent image pixels. Native capture continues to
use the installed Ghostty application for renderer- and image-specific evidence.

## State boundaries

A state is emitted when visible or semantic terminal content changes, including:

- cells, graphemes, styles, and resolved colors;
- cursor position and shape;
- terminal modes and active style;
- title, working directory, palette, or geometry.

Each state records its event sequence and byte offset. Input, signals, markers, and lifecycle events remain in the timeline but do not invent visual changes.

The first record contains a complete grid. Later records contain changed metadata and cells grouped by row. `state-index.jsonl` provides a lightweight event and time lookup without expanding every delta.

State hashes commit to the complete viewport but exclude transient dirty bookkeeping. Dirty rows remain available as diagnostic evidence.

## Synchronized output

DEC mode 2026 groups terminal changes into one presented update. Termtrace retains intermediate parser states with `presented: false` and marks the committed state as presented.

This distinction lets protocol debugging inspect hidden work without claiming that a user saw it. Use `index --presented-only` when those intermediate states are unnecessary.

## Select a range

Use time offsets or unique markers to replay part of a session:

```bash
termtrace analyze index --session SESSION --from 30s --to 45s
termtrace analyze index --session SESSION --from-marker start --to-marker finish
```

Ranges are half-open: the start is included and the end is excluded. Replay processes the earlier prefix to reconstruct the correct starting state but does not emit prefix states. The range begins with a complete state at its inclusive boundary.

Ranged manifests retain absolute source times and the exact selected bounds. Invalid, empty, or out-of-session ranges fail instead of being silently adjusted.

## Inspect a point

Completed sessions can resolve a state, event, time, or marker:

```sh
termtrace show SESSION --state 42
termtrace show SESSION --state-hash HASH
termtrace show SESSION --event-seq 120
termtrace show SESSION --at 12.5s
termtrace show SESSION --at-marker ready
```

The default selects the latest presented state. Explicit hidden states require `--include-hidden`.

A state hash selects semantic content across `inspect`, `show`, `save`,
`screenshot`, `diff`, and `explain`. When the same content recurs, the latest
matching canonical occurrence supplies the source boundary.

One-off completed observations publish compressed disposable checkpoints. They are bounded to 32 entries and 64 MiB per session and never replace canonical evidence:

```bash
termtrace analyze cache status SESSION
termtrace analyze cache prune SESSION
```

## Semantic and native views

Semantic state is independent of video sampling. A state that lasts less than one video frame remains inspectable.

Semantic PNG and SVG rendering are deterministic derived views. Native PNG explicitly launches the installed terminal application and never falls back to semantic rendering. Structured output labels the source and capture quality so consumers can distinguish them.

## Semantic video

Presentation planning, preview, and export use verified native Ghostty video by default. Select semantic rendering explicitly when a deterministic MP4 is needed on a host without the native display stack:

```bash
termtrace video SESSION --renderer semantic --fps 24 --edit EDIT.json --dry-run
termtrace video SESSION --renderer semantic --edit EDIT.json --preview-out PREVIEW
termtrace video SESSION --renderer semantic --edit EDIT.json --out EXPORT.mp4
```

The semantic scheduler samples the latest presentation-eligible state at each frame boundary. It excludes synchronized-output hidden states and records states too brief to receive a frame as suppressed. A final source frame retains the completed semantic state. Resized sessions use one fixed maximum-grid canvas.

Semantic video is labeled `semantic_rendering`. It visualizes terminal state with Termtrace's bundled deterministic renderer; it is not evidence of pixels drawn by Ghostty. Native remains the default and should be used for font, shaping, clipping, shader, image, or renderer-specific investigations. Source selection never falls back automatically.
