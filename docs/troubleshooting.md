# Troubleshooting

## Build says libghostty-vt is missing

Run `scripts/bootstrap.sh` from the repository root. It initializes the Ghostty submodule, provisions the pinned Zig toolchain, and builds libghostty-vt. An external build can be selected with `TERMTRACE_GHOSTTY_PREFIX`, but it must provide compatible `include/ghostty/vt.h` and `lib/libghostty-vt.a`; compatibility is intentionally not shimmed.

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

Recordings made with fitted geometry or `--ghostty-config`/visual convenience flags contain their canonical effective Ghostty profile, so changing or deleting the original config file does not affect capture. The referenced fonts, shaders, background images, and other external resources must still be installed. Legacy or manual recordings without a saved profile continue to validate the current user config and stop if it changed.

If capture reports a geometry mismatch, the recorded grid does not fit the selected viewport with the available Ghostty font metrics. Prefer recording with `--geometry fit --viewport WIDTHxHEIGHT`. For a fitted recording, confirm that the font and other external profile resources still exist, or make a new recording rather than overriding the viewport until wrapping happens to look close.

## Video does not show every transient state

At 24 FPS, a frame spans roughly 41.7 ms. This limits the video only. `states.jsonl` remains byte-granular, including mode-2026 suppressed states, and `video-map.jsonl` marks non-presented states with a null frame.
