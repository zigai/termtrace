export TERMTRACE_GHOSTTY_PREFIX := justfile_directory() + "/vendor/ghostty/zig-out"

_:
  @just help

# Prepare the pinned semantic engine and native build dependencies
bootstrap:
  scripts/bootstrap.sh

# Install Termtrace from this checkout
install: bootstrap
  cargo install --locked --path . --force

# Regenerate public schemas
generate:
  cargo run --locked --quiet -- schema --out schemas

# Enforce the complete allowlist of direct internal Cargo dependencies
architecture:
  python3 scripts/check-architecture.py
  python3 -m unittest discover -s scripts -p test_check_architecture.py

# Compile every Rust target and feature without running tests
validate:
  cargo check --locked --workspace --all-targets --all-features

# Run fast hermetic Rust tests for each change (requires bootstrap)
test:
  cargo nextest run --locked --workspace --lib --bins --test execution_lifecycle
  PYTHONPATH=benchmarks uv run --project benchmarks python3 -m unittest benchmarks/test_acceptance.py

# Run CLI/tmux integration on each pull request (requires bootstrap, tmux, and Ghostty on PATH)
integration:
  cargo test --locked --test cli

# Build native Python and Node.js language bindings
bindings-build:
  uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml
  bunx @napi-rs/cli build --cwd crates/bindings-node

# Test native Python language bindings and pytest consumer suite
test-python:
  uv run --project tests/consumers/python_native pytest -v tests/consumers/python_native

# Test native Node.js/TypeScript bindings and consumer suite
test-node:
  npm --prefix tests/consumers/node_native test

# Test all native language bindings
bindings-test: bindings-build test-python test-node

# Lint Rust
lint:
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# Format Rust and shell scripts
format:
  cargo statement-spacing fix --format-first
  shfmt -i 4 -ci -w benchmarks examples fixtures scripts tests install.sh

# Run the complete native recording, replay, video, and screenshot verification test
native-check:
  @scripts/native-capture-check.sh

# Run performance and resource benchmarks on demand (requires native capture tooling)
bench:
  @benchmarks/run-agent-workflows.sh
  @benchmarks/run-durability.sh
  @benchmarks/run-render.sh

# Synchronize skill definition to local skills catalog if present
sync-skill:
  @scripts/sync-skill.sh

# Auto-fix linting and formatting issues
fix: sync-skill
  cargo statement-spacing fix --format-first
  cargo clippy --locked --workspace --all-targets --all-features --fix --allow-dirty --allow-staged
  @just format

# Run tests and display coverage
coverage:
  cargo llvm-cov --locked --workspace --html


# Replay native Ghostty VT seeds; pass libFuzzer arguments to explore (requires cargo-fuzz and nightly-2026-08-25 with rust-src)
[positional-arguments]
fuzz-vt *args="-runs=0":
  cargo +nightly-2026-08-25 fuzz run ghostty_vt fuzz/corpus/ghostty_vt -- -timeout=5 -max_len=4096 "$@"

# Run concurrency checks under ThreadSanitizer (requires nightly-2026-08-25 with rust-src)
test-race:
  RUSTFLAGS="-Zsanitizer=thread" cargo +nightly-2026-08-25 test --locked -Zbuild-std --target x86_64-unknown-linux-gnu -p termtrace-engine connection_limiter

# Audit Rust dependencies for security advisories
security:
  cargo audit --file Cargo.lock
  cargo audit --file fuzz/Cargo.lock
# Run required local checks for each change (requires bootstrap and shfmt)
check: architecture lint validate test
  cargo statement-spacing check
  shfmt -i 4 -ci -d benchmarks examples fixtures scripts tests install.sh

# List available commands
help:
  @just --list

alias fmt := format
alias cov := coverage
