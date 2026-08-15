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

# run Rust, TypeScript, and Python tests
test:
  cargo test --locked --workspace --all-features
  bun run --cwd packages/typescript test
  uv run --project packages/python --locked --group dev pytest -q packages/python/tests

# lint Rust and type-check and lint the language clients
lint:
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  bun run --cwd packages/typescript lint
  bun run --cwd packages/typescript typecheck
  uv run --project packages/python --locked --group dev mypy packages/python/termtrace packages/python/tests
  uv run --project packages/python --locked --group dev ruff check packages/python/termtrace packages/python/scripts packages/python/tests

# format Rust and language-client sources
format:
  cargo fmt --all
  bun run --cwd packages/typescript format
  uv run --project packages/python --locked --group dev ruff format packages/python/termtrace packages/python/scripts packages/python/tests

# verify formatting without modifying files
format-check:
  cargo fmt --all -- --check
  bun run --cwd packages/typescript format:check
  uv run --project packages/python --locked --group dev ruff format --check packages/python/termtrace packages/python/scripts packages/python/tests

# run the complete native recording, replay, video, and screenshot smoke test
smoke:
  @scripts/native-smoke-test.sh

# run every non-mutating repository check
check: architecture format-check lint validate test

# list available commands
help:
  @just --list

alias fmt := format
