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

# compile every Rust target and feature without running tests
validate:
  cargo check --locked --all-targets --all-features

# run Rust, TypeScript, and Python tests
test:
  cargo test --locked --all-features
  bun test --cwd packages/typescript
  uv run --project packages/python --locked --group dev pytest -q packages/python/tests

# lint Rust and type-check and lint the language clients
lint:
  cargo clippy --locked --all-targets --all-features -- -D warnings
  bun run --cwd packages/typescript check
  uv run --project packages/python --locked --group dev mypy packages/python/src packages/python/tests
  uv run --project packages/python --locked --group dev ruff check --force-exclude packages/python/src/termtrace packages/python/scripts packages/python/tests --exclude packages/python/src/termtrace/_generated

# format Rust and handwritten Python sources
format:
  cargo fmt --all
  uv run --project packages/python --locked --group dev ruff format --force-exclude packages/python/src/termtrace packages/python/scripts packages/python/tests --exclude packages/python/src/termtrace/_generated

# verify formatting without modifying files
format-check:
  cargo fmt --all -- --check
  uv run --project packages/python --locked --group dev ruff format --check --force-exclude packages/python/src/termtrace packages/python/scripts packages/python/tests --exclude packages/python/src/termtrace/_generated

# run the complete native recording, replay, video, and screenshot smoke test
smoke:
  @scripts/native-smoke-test.sh

# run every non-mutating repository check
check: format-check lint validate test

# list available commands
help:
  @just --list

alias fmt := format
