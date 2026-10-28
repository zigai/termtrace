#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

echo "=== 1. Building Native Python Extension ==="
uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml

echo "=== 2. Running Python Type & Lint Checks ==="
uv run --project crates/bindings-python ruff check crates/bindings-python/python/termtrace
uv run --project crates/bindings-python mypy crates/bindings-python/python/termtrace

echo "=== 3. Running Python Consumer Test Suite ==="
uv run --project tests/consumers/python_native pytest -v tests/consumers/python_native
uv run --project tests/consumers/python_native pytest -v examples/sdk/python

echo "=== 4. Building Native Node Extension ==="
bunx @napi-rs/cli build --cwd crates/bindings-node

echo "=== 5. Running Node Helper Units, TypeScript Examples, and Consumer Suite ==="
node --test crates/bindings-node/test/*.test.js
npm --prefix tests/consumers/node_native test

echo "=== All Native Language Bindings Verified Successfully ==="
