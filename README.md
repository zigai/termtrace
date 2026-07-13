# Termtrace

Termtrace records terminal and TUI sessions as two synchronized sources of evidence:

- native Ghostty screenshots and 24 FPS video for human visual review;
- a byte-exact event log replayed through `libghostty-vt` for agent-readable terminal states.

The raw event log is authoritative. Videos, screenshots, text, JSON, VT, and HTML views are derived artifacts that can be regenerated after a session exits.

> [!WARNING]
> Terminal recordings can contain prompts, source code, environment details, credentials, clipboard data, and typed secrets. Session directories are owner-only by default and must be treated as sensitive.

## Status

Termtrace is under active development. The architecture and acceptance criteria are documented in [Architecture](docs/architecture.md).

The functional PTY proxy supports both interactive and non-interactive commands:

```bash
cargo run -- record \
  --out /tmp/example.termtrace \
  --cols 160 --rows 40 \
  --cell-width 9 --cell-height 18 \
  -- your-command --argument
```

Command output is mirrored unchanged. Standard input is forwarded in raw mode, outer-terminal resizes are propagated, and host terminal settings are restored on every normal and error path. Use `--no-input` for a detached producer. The child receives a real PTY with `TERM=xterm-ghostty` and `COLORTERM=truecolor`; the command's exit status is propagated.

The new owner-only session directory contains an integrity-chained `events.jsonl` recording with raw bytes encoded as Base64. Inputs and resizes are represented by durable intent/result pairs. A crash between the pair is reported as uncertain, never silently retried. High-volume output is buffered between durable control boundaries so recording does not add an `fsync` pause to every PTY read.

Termtrace refuses to reuse an existing session directory. This prevents accidental evidence loss and makes every run independently auditable.

## Development

The repository pins its Rust toolchain. Validate a change with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
```
