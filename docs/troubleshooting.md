# Troubleshooting

## Build says libghostty-vt is missing

Initialize submodules and run `scripts/bootstrap-ghostty-vt.sh`. An external build can be selected with `TERMTRACE_GHOSTTY_PREFIX`, but it must provide compatible `include/ghostty/vt.h` and `lib/libghostty-vt.a`; compatibility is intentionally not shimmed.

## A control command cannot connect

Confirm `control.sock` exists, inspect the recorder process, then run `termtrace ping`. The socket is removed after child exit. Linux `SO_PEERCRED` rejects a different UID even if filesystem permissions were bypassed.

## Wait reports a temporary invalid tail

Active buffered writers can expose a short read race. `termtrace wait` retries this only while the control socket remains responsive. Completed invalid logs fail validation and should be recovered explicitly.

## Recover a damaged log

```bash
mkdir -m 700 recovered
termtrace recover --event-log SESSION/events.jsonl --out recovered/events.jsonl
```

Recovery never modifies the source. It copies only the longest hash-checked, schema-valid, newline-complete prefix and reports the discarded line plus any intent whose delivery remains uncertain.

## Native capture fails

Termtrace preserves a `visual-failed-*` owner-only directory. Inspect `ghostty.stderr.log`, `xvfb.log`, and `ffmpeg-video.log`. Common causes are a missing x11grab/libx264rgb FFmpeg build, unavailable software OpenGL, missing fonts referenced by Ghostty config, or desktop portal startup failures.

The capture manifest includes Ghostty and FFmpeg versions, the resolved Ghostty configuration and hash, exact launch overrides, and FFprobe evidence. A successful process exit without valid probe evidence is still a failure.

## Video does not show every transient state

At 24 FPS, a frame spans roughly 41.7 ms. This limits the video only. `states.jsonl` remains byte-granular, including mode-2026 suppressed states, and `video-map.jsonl` marks non-presented states with a null frame.
