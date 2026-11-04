_:
  @just help

# Prepare the pinned semantic engine and native build dependencies
setup:
  scripts/bootstrap.sh

# Build a debug version
build: setup
  cargo build --locked

# Install Termtrace from this checkout
install: setup
  cargo install --locked --path . --force

# Regenerate public schemas and the generated Python and TypeScript SDK sources
generate:
  cargo run --locked --quiet -- schema --out schemas
  cargo run --locked --quiet -p termtrace-python --bin stub_gen
  python3 scripts/sdk-types.py
  python3 scripts/sdk-aio.py
  uv run --project crates/bindings-python ruff check --fix --quiet crates/bindings-python/python/termtrace/types.py crates/bindings-python/python/termtrace/aio.py crates/bindings-python/python/termtrace/_termtrace.pyi
  uv run --project crates/bindings-python ruff format crates/bindings-python/python/termtrace/types.py crates/bindings-python/python/termtrace/aio.py crates/bindings-python/python/termtrace/_termtrace.pyi

# Enforce the complete allowlist of direct internal Cargo dependencies
architecture:
  cargo test --locked --test architecture

# Compile every Rust target and feature without running tests
validate:
  cargo check --locked --workspace --all-targets --all-features

# Run fast hermetic Rust tests for each change (requires bootstrap)
test:
  cargo nextest run --locked --workspace --lib --bins --test execution_lifecycle --test architecture
  cargo test --locked -p termtrace-sdk --no-default-features
  PYTHONPATH=benchmarks uv run --project benchmarks python3 -m unittest benchmarks/test_acceptance.py
  bash tests/install.sh

# Run CLI/tmux integration on each pull request (requires bootstrap, tmux, and Ghostty on PATH)
integration:
  cargo test --locked --test cli
  scripts/ci-rust-consumer.sh

# Build native Python and Node.js language bindings
bindings-build:
  uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml
  bunx @napi-rs/cli build --cwd crates/bindings-node

# Test native Python language bindings and pytest consumer suite
test-python:
  uv run --project tests/consumers/python_native pytest -v tests/consumers/python_native
  TERMTRACE_BIN={{justfile_directory()}}/target/debug/termtrace uv run --project tests/consumers/python_native pytest -v examples/sdk/python

# Test native Node.js/TypeScript bindings, the type-checked examples, and consumer suite
test-node:
  node --test crates/bindings-node/test/*.test.js
  TERMTRACE_BIN={{justfile_directory()}}/target/debug/termtrace npm --prefix tests/consumers/node_native test

# Type-check and lint the native Python bindings
bindings-lint:
  uv run --project crates/bindings-python ruff check crates/bindings-python/python/termtrace
  uv run --project crates/bindings-python mypy crates/bindings-python/python/termtrace

# Test all native language bindings
bindings-test: bindings-build bindings-lint test-python test-node

# Lint Rust, shell, workflow pins, and formatting
lint:
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  cargo clippy --locked -p termtrace-sdk --no-default-features --all-targets -- -D warnings
  cargo fmt --all -- --check
  cargo fmt --manifest-path tests/consumers/rust/Cargo.toml -- --check
  cargo statement-spacing check
  cargo machete
  shfmt -i 4 -ci -d benchmarks examples fixtures scripts tests install.sh
  for file in install.sh benchmarks/*.sh scripts/*.sh examples/*.sh; do bash -n "$file"; done
  python3 -m py_compile examples/apps/terminal_showcase.py
  bash scripts/check-github-action-pins.sh

# Format Rust and shell scripts
format:
  cargo statement-spacing fix --format-first
  shfmt -i 4 -ci -w benchmarks examples fixtures scripts tests install.sh

# Run the complete native recording, replay, video, and screenshot verification test
native-check:
  @scripts/native-capture-check.sh

# Exercise native capability failures, retained export, and original capture (requires Ghostty, Xvfb, dbus-run-session, ImageMagick, and FFmpeg)
native-test:
  bash scripts/native-capability-test.sh
  cargo test --locked --test cli retained_native_video_exports_without_native_capture_tools -- --ignored
  cargo test --locked --test cli original_native -- --ignored
  cargo test --locked -p termtrace-service --test original_recording -- --ignored --test-threads=1
  TERMTRACE_BIN={{justfile_directory()}}/target/debug/termtrace cargo test --locked --manifest-path tests/consumers/rust/Cargo.toml --features local --tests -- --ignored --test-threads=1
  scripts/sdk-demos.sh
  TERMTRACE_BIN={{justfile_directory()}}/target/debug/termtrace bash examples/demo.sh "$(mktemp -d -p target demo.XXXXXX)"

# Certify every SDK capability row against executed Rust, Python, and TypeScript tests (requires native tools)
conformance:
  scripts/ci-sdk-conformance.sh

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


# Replay native Ghostty VT seeds; pass libFuzzer arguments to explore (requires cargo-fuzz and nightly-2026-08-20 with rust-src)
[positional-arguments]
fuzz-vt *args="-runs=0":
  cargo +nightly-2026-08-20 fuzz run ghostty_vt fuzz/corpus/ghostty_vt -- -timeout=5 -max_len=4096 "$@"

# Run one-minute fuzz campaigns and replay native VT seeds (requires cargo-fuzz and nightly-2026-08-20 with rust-src)
fuzz: fuzz-vt
  cargo +nightly-2026-08-20 fuzz run output_projection fuzz/corpus/output_projection -- -max_total_time=60 -timeout=5 -max_len=4096
  cargo +nightly-2026-08-20 fuzz run control_protocol fuzz/corpus/control_protocol -- -max_total_time=60 -timeout=5 -max_len=16384
  cargo +nightly-2026-08-20 fuzz run event_log fuzz/corpus/event_log -- -max_total_time=60 -timeout=5 -max_len=262144
  cargo +nightly-2026-08-20 fuzz run bundle_archive fuzz/corpus/bundle_archive -- -max_total_time=60 -timeout=5 -max_len=262144

# Check model undefined behavior under Miri (requires nightly-2026-08-20 with miri)
miri:
  cargo +nightly-2026-08-20 miri test --locked -p termtrace-model --lib

# Run the terminal boundary under AddressSanitizer against a sanitized libghostty-vt (requires nightly-2026-08-20 with rust-src)
sanitize:
  TERMTRACE_SANITIZE=address TERMTRACE_GHOSTTY_PREFIX={{justfile_directory()}}/target/ghostty-asan scripts/bootstrap-ghostty-vt.sh
  ASAN_OPTIONS=detect_leaks=1 RUSTFLAGS=-Zsanitizer=address TERMTRACE_GHOSTTY_PREFIX={{justfile_directory()}}/target/ghostty-asan cargo +nightly-2026-08-20 test --locked -Zbuild-std --target x86_64-unknown-linux-gnu -p termtrace-terminal --lib

# Run concurrency checks under ThreadSanitizer (requires nightly-2026-08-20 with rust-src)
race:
  RUSTFLAGS="-Zsanitizer=thread" cargo +nightly-2026-08-20 test --locked -Zbuild-std --target x86_64-unknown-linux-gnu -p termtrace-engine connection_limiter

# Audit Rust dependencies for security advisories
audit:
  cargo audit --file Cargo.lock
  cargo audit --file fuzz/Cargo.lock

# Kill mutants in the streaming control stripper and screen boundary
mutation:
  TERMTRACE_GHOSTTY_PREFIX={{justfile_directory()}}/vendor/ghostty/zig-out cargo mutants --cargo-arg=--locked -p termtrace-terminal -f crates/terminal/src/output_projection.rs -f crates/terminal/src/screen.rs --timeout 60 --jobs 2

# Run every repository check, including native capture, fuzzing, sanitizers, Miri, and mutation testing (requires bootstrap and the tools each recipe names)
check: lint validate test integration bindings-test native-check native-test audit fuzz miri sanitize race mutation

# List available commands
help:
  @just --list

alias fmt := format
alias cov := coverage
