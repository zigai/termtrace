# Troubleshooting

Start with the capability report and the diagnostic code from the failing command:

```bash
termtrace doctor --explain
```

## Build cannot find the semantic library

Run the bootstrap from the repository root:

```bash
scripts/bootstrap.sh
```

It initializes the submodule, installs the pinned Zig toolchain, and builds the required library. Use `TERMTRACE_GHOSTTY_PREFIX` only for an intentionally compatible external build.

## A control command cannot connect

Check that the session is still live:

```bash
termtrace ping SESSION
termtrace status SESSION
```

`control.sock` disappears after the child exits. The socket also rejects clients owned by a different user.

If a friendly name is involved, inspect all registry states:

```bash
termtrace list --all --json
termtrace resolve NAME --json
```

## A wait times out

Choose the predicate that matches the evidence you need:

```bash
# Current presented screen
termtrace wait SESSION 'Ready' --stable-for 250ms

# Any canonical output history
termtrace wait SESSION --output 'earlier diagnostic'
```

Screen waits ignore hidden synchronized-output states. Output waits search the event history, including text split across PTY reads.

## The event log has a damaged tail

Recover the longest valid prefix into a new directory:

```bash
mkdir -m 700 recovered
termtrace recover \
  --event-log SESSION/events.jsonl \
  --out recovered/events.jsonl
```

Recovery never modifies the source. Its report identifies discarded data and any mutation whose delivery is uncertain.

## Index says the session is incomplete

Visible final text does not mean the recording is complete. Wait for a durable completion manifest:

```bash
termtrace stop SESSION --wait
# or, without sending a signal:
termtrace status SESSION --wait-complete
```

Then run `index` again. Use `index --live-snapshot` only when an explicitly incomplete prefix is intended. Native capture requires completed evidence.

For ranged capture, first create an index with exactly the same `--from` and `--to` values or marker selectors.

## Native capture fails

Termtrace preserves a `visual-failed-*` directory. Inspect the structured geometry or playback error first, then the component logs:

- `geometry-preflight.json`
- `playback-error.json`
- `ghostty.stderr.log`
- `xvfb.log`
- `ffmpeg-video.log`

Common causes are missing fonts, unavailable software OpenGL, a missing codec, or desktop-session startup failure.

If geometry does not match, record with a fitted viewport:

```bash
termtrace record --out SESSION --geometry fit --viewport 1920x1080 COMMAND
```

`--width` and `--height` refer to the complete capture viewport, not only the text grid. Fitted recordings preserve their flattened terminal configuration, but external fonts, shaders, and images must still be installed.

## The command starts in the wrong directory

Without `--cwd`, the child inherits the directory where `termtrace record` was invoked. Set it explicitly when needed:

```bash
termtrace record --out SESSION --cwd ./project COMMAND
```

Inspect `working_directory` in `session.json` to see the resolved value.

## Video misses a brief state

Native video runs at 24 FPS, so a frame spans about 41.7 ms. Semantic replay is not frame-limited: brief states remain in `states.jsonl.zst` and can be inspected or rendered separately.

States hidden by synchronized output are retained with `presented: false`. Use `index --presented-only` when they are not needed.

Semantic video also uses a fixed frame rate and honestly reports presentation-eligible states that receive no sampled frame. Use the semantic timeline or render an exact state screenshot when every brief transition matters; increasing `--fps` can reduce, but not eliminate, sampling loss.

## Semantic video export fails

Check the independent semantic-video and presentation capabilities:

```bash
termtrace doctor --require semantic-video,presentation --explain
```

Semantic source failures retain `semantic-source-ffmpeg.log` in the failed presentation staging directory when available. They do not modify canonical evidence or silently retry with native capture.
