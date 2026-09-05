_:
  @just help

# prepare the pinned semantic engine and native build dependencies
bootstrap:
  scripts/bootstrap.sh

# install Termtrace from this checkout
install: bootstrap
  cargo install --locked --path . --force

# regenerate public schemas and typed client protocol models
generate:
  cargo run --locked --quiet -- tooling schemas --out schemas
  scripts/generate-client-protocol.sh

# enforce the complete allowlist of direct internal Cargo dependencies
architecture:
  python3 scripts/check-architecture.py

# compile every Rust target and feature without running tests
validate:
  cargo check --locked --workspace --all-targets --all-features

# run fast hermetic Rust, TypeScript, and Python tests for each change (requires bootstrap, Bun, and uv)
test:
  cargo test --locked --workspace --lib --bins
  bun run --cwd packages/typescript test
  uv run --project packages/python --locked --group dev pytest -q packages/python/tests

# run CLI/tmux integration on each pull request (requires bootstrap, tmux, and Ghostty on PATH)
integration:
  cargo test --locked --test cli

# lint Rust and type-check and lint the language clients
lint:
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  bun run --cwd packages/typescript lint
  bun run --cwd packages/typescript typecheck
  uv run --project packages/python --locked --group dev mypy packages/python/termtrace packages/python/tests
  uv run --project packages/python --locked --group dev ruff check packages/python/termtrace packages/python/scripts packages/python/tests

# format Rust, language-client sources, and shell scripts
format:
  cargo fmt --all
  bun run --cwd packages/typescript format
  uv run --project packages/python --locked --group dev ruff format packages/python/termtrace packages/python/scripts packages/python/tests
  shfmt -i 4 -ci -w benchmarks examples fixtures scripts tests install.sh

# verify formatting without modifying files
format-check:
  cargo fmt --all -- --check
  bun run --cwd packages/typescript format:check
  uv run --project packages/python --locked --group dev ruff format --check packages/python/termtrace packages/python/scripts packages/python/tests
  shfmt -i 4 -ci -d benchmarks examples fixtures scripts tests install.sh

# run the complete native recording, replay, video, and screenshot smoke test
smoke:
  @scripts/native-smoke-test.sh

# run performance and resource benchmarks on demand (requires native capture tooling)
bench:
  @benchmarks/run-agent-workflows.sh
  @benchmarks/run-durability.sh
  @benchmarks/run-render.sh

# run required local checks for each change (requires bootstrap, Bun, uv, and shfmt)
check: architecture format-check lint validate test

# list available commands
help:
  @just --list

alias fmt := format
