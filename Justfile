export TERMTRACE_GHOSTTY_PREFIX := justfile_directory() + "/vendor/ghostty/zig-out"

_:
  @just help

# Prepare the pinned semantic engine and native build dependencies
bootstrap:
  scripts/bootstrap.sh

# Install Termtrace from this checkout
install: bootstrap
  cargo install --locked --path . --force

# Regenerate public schemas and typed client protocol models
generate:
  cargo run --locked --quiet -- tooling schemas --out schemas
  scripts/generate-client-protocol.sh

# Enforce the complete allowlist of direct internal Cargo dependencies
architecture:
  python3 scripts/check-architecture.py

# Compile every Rust target and feature without running tests
validate:
  cargo check --locked --workspace --all-targets --all-features

# Run fast hermetic Rust, TypeScript, and Python tests for each change (requires bootstrap, Bun, and uv)
test:
  cargo test --locked --workspace --lib --bins --test execution_lifecycle
  bun run --cwd packages/typescript test
  uv run --project packages/python --locked --group dev pytest -q packages/python/tests benchmarks/test_acceptance.py

# Run CLI/tmux integration on each pull request (requires bootstrap, tmux, and Ghostty on PATH)
integration:
  cargo test --locked --test cli

# Lint Rust and type-check and lint the language clients
lint:
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  bun run --cwd packages/typescript lint
  bun run --cwd packages/typescript typecheck
  uv run --project packages/python --locked --group dev mypy packages/python/termtrace packages/python/tests
  uv run --project packages/python --locked --group dev ruff check packages/python/termtrace packages/python/scripts packages/python/tests

# Format Rust, language-client sources, and shell scripts
format:
  cargo fmt --all
  bun run --cwd packages/typescript format
  uv run --project packages/python --locked --group dev ruff format packages/python/termtrace packages/python/scripts packages/python/tests
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
  bun run --cwd packages/typescript lint:fix
  uv run --project packages/python --locked --group dev ruff check --fix packages/python/termtrace packages/python/scripts packages/python/tests
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

# Audit Rust, TypeScript, and Python dependencies for security advisories
security:
  cargo audit --file Cargo.lock
  cargo audit --file fuzz/Cargo.lock
  bun --cwd packages/typescript audit
  uv run --project packages/python --locked --group dev --with pip-audit pip-audit
# Run required local checks for each change (requires bootstrap, Bun, uv, and shfmt)
check: architecture lint validate test
  cargo fmt --all -- --check
  cargo statement-spacing check
  bun run --cwd packages/typescript format:check
  uv run --project packages/python --locked --group dev ruff format --check packages/python/termtrace packages/python/scripts packages/python/tests
  shfmt -i 4 -ci -d benchmarks examples fixtures scripts tests install.sh

# List available commands
help:
  @just --list

alias fmt := format
alias cov := coverage
