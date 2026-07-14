# Troubleshooting

## Build says libghostty-vt is missing

Run `scripts/bootstrap.sh` from the repository root. It initializes the Ghostty submodule, provisions the pinned Zig toolchain, and builds libghostty-vt. An external build can be selected with `TERMTRACE_GHOSTTY_PREFIX`, but it must provide compatible `include/ghostty/vt.h` and `lib/libghostty-vt.a`; compatibility is intentionally not shimmed.

## A control command cannot connect

Confirm `control.sock` exists, inspect the recorder process, then run `termtrace ping`. The socket is removed after child exit. Linux `SO_PEERCRED` rejects a different UID even if filesystem permissions were bypassed.

## Wait reports a temporary invalid tail

Use positional text or `--screen-absent` for the latest presented viewport; these sample the actor's presentation-filtered state and can require `--stable-for`. Use `--output` only for canonical output history. Active buffered writers can expose a short read race during output-history reads; completed invalid logs fail validation and should be recovered explicitly.

## Recover a damaged log

```bash
mkdir -m 700 recovered
termtrace recover --event-log SESSION/events.jsonl --out recovered/events.jsonl
```

Recovery never modifies the source. It copies only the longest hash-checked, schema-valid, newline-complete prefix and reports the discarded line plus any intent whose delivery remains uncertain.

## Native capture fails

Termtrace preserves a `visual-failed-*` owner-only directory. Inspect `ghostty.stderr.log`, `xvfb.log`, and `ffmpeg-video.log`. Common causes are a missing x11grab/libx264rgb FFmpeg build, unavailable software OpenGL, missing fonts referenced by Ghostty config, or desktop portal startup failures.

The capture manifest includes Ghostty and FFmpeg versions, the resolved Ghostty configuration and hash, exact launch overrides, a parent-side `geometry-preflight.json` report, and FFprobe evidence. A successful process exit without valid probe evidence is still a failure. Playback writes `playback-error.json` before an unsuccessful child exit, and the parent watches that handshake alongside timing so the root cause is reported without waiting for a generic timeout.

Recordings made with fitted geometry or `--ghostty-config`/visual convenience flags contain their canonical effective Ghostty profile, so changing or deleting the original config file does not affect capture. The referenced fonts, shaders, background images, and other external resources must still be installed. Legacy or manual recordings without a saved profile continue to validate the current user config and stop if it changed.

If capture reports a geometry mismatch, the recorded grid does not fit the selected viewport with the available Ghostty font metrics. Prefer recording with `--geometry fit --viewport WIDTHxHEIGHT`. For a fitted recording, confirm that the font and other external profile resources still exist, or make a new recording rather than overriding the viewport until wrapping happens to look close.

`--width` and `--height` describe the whole capture viewport, not the terminal text area. The preflight diagnostic reports both the actual grid/text pixels and the recorded grid. Portal, AT-SPI, and ignored runtime-resize warnings may also appear in successful captures; use the structured preflight/playback error rather than treating those warnings as the cause.

## Index reports an incomplete or stale session

Seeing expected screen text does not complete a recording. Run `termtrace stop SESSION --wait` or `termtrace status SESSION --wait-complete` before normal indexing. Use `index --live-snapshot` only when an explicitly incomplete prefix is intended; its source-hash suffix prevents it from occupying the final derived directory, and capture refuses incomplete provenance. If capture names a provenance mismatch, remove the named final derived directory and regenerate it from the completed log.

For a ranged capture, create the semantic index with exactly the same `--from` and `--to` values first. Bounds are resolved to absolute nanoseconds and select `[from, to)`; the error names the matching index command and deterministic directory. Do not rename a range directory or reuse an index from different bounds. If fast-forward geometry verification fails, inspect pre-range resize intent/result events rather than increasing delays: capture uses a terminal-query barrier and does not guess that Ghostty caught up.

## A recorded command starts in the wrong directory

Without `--cwd`, current Termtrace releases inherit the directory from which `termtrace record` was invoked and persist its resolved path in `session.json`. `--cwd` overrides this, with relative paths resolved against that same caller directory. Inspect `working_directory` in `session.json`; it is Base64-backed byte-string JSON so non-UTF-8 paths remain lossless. Older recordings may contain `null` because earlier releases delegated the unset directory to `portable-pty`, which selected the user's home directory.

## Video does not show every transient state

At 24 FPS, a frame spans roughly 41.7 ms. This limits the video only. `states.jsonl.zst` remains byte-granular, including mode-2026 suppressed states, and `video-map.jsonl` marks non-presented states with a null frame. Use `index --presented-only` when those suppressed derived records are not needed.
