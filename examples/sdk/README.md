# SDK examples

The same two programs in Rust, Python, and TypeScript. The consumer test suites run these
files, so they stay in sync with the SDKs.

## Ordinary test workflow

Launches `fixtures/programs/terminal/line-command.sh` at 80x24, waits for `LINE_READY`, sends
`hello` and Enter guarded by the ready state, asserts `LINE_VALUE=hello`, compares the reviewed
snapshot `snapshots/line-value.json`, finalizes, and reads the retained evidence.

- Rust: [rust/workflow.rs](rust/workflow.rs), outer check [tests/consumers/rust/tests/workflow.rs](../../tests/consumers/rust/tests/workflow.rs)
- Python: [python/test_workflow.py](python/test_workflow.py)
- TypeScript: [typescript/workflow.test.ts](typescript/workflow.test.ts)

```sh
cargo build --locked
export TERMTRACE_BIN=$PWD/target/debug/termtrace
cargo test --locked --manifest-path tests/consumers/rust/Cargo.toml --features local --test workflow
uv run --project tests/consumers/python_native pytest examples/sdk/python
npm --prefix tests/consumers/node_native test
```

The TypeScript files compile with the consumer's locked `tsc`
(`tests/consumers/node_native/tsconfig.examples.json`) before they run.

Set `TERMTRACE_WORKFLOW_MISMATCH=1` to expect `LINE_VALUE=goodbye` instead. The run then fails
and prints one `TERMTRACE_WORKFLOW_FAILURE` JSON line with the failed assertion, the session
path, the last screen text, and the private evidence directory. Each suite runs this mode as a
subprocess and checks that report. Set `TERMTRACE_UPDATE_SNAPSHOTS=1` only to rewrite the
reviewed snapshot on purpose.

## Edited demo

Records `examples/apps/terminal_showcase.py` once with original Ghostty pixels and a fixed
visual profile (JetBrains Mono 12 pt, 96 DPI, 80x24 in a 1280x720 canvas at 24 FPS), marks it,
sends input and signals, then renders a preview, MP4, GIF, and WebM of
`[showcase-ready, signals-observed)` from that one recording. It checks that every output comes
from the original capture and that the canonical session files are unchanged.

- Rust: [rust/demo.rs](rust/demo.rs)
- Python: [python/demo.py](python/demo.py)
- TypeScript: [typescript/demo.ts](typescript/demo.ts)

```sh
scripts/sdk-demos.sh target/sdk-demos/run
```

Requires Ghostty, Xvfb, `dbus-run-session`, FFmpeg with libx264 and libvpx-vp9, and the
JetBrains Mono font. The script checks `termtrace doctor` first and fails when a tool is
missing. Outputs contain terminal content; review them before sharing.
