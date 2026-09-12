#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bun install --cwd packages/typescript --frozen-lockfile
scripts/generate-client-protocol.sh
git diff --exit-code -- \
    packages/python/termtrace/transport/generated \
    packages/typescript/src/transport/generated

bun run --cwd packages/typescript format:check
bun run --cwd packages/typescript lint
bun run --cwd packages/typescript typecheck
bun run --cwd packages/typescript test
node --check packages/typescript/bin/termtrace.js

uv run --project packages/python --locked --group dev \
    pytest -q packages/python/tests benchmarks/test_acceptance.py
uv run --project packages/python --locked --group dev \
    mypy packages/python/termtrace packages/python/tests
uv run --project packages/python --locked --group dev \
    ruff check packages/python/termtrace packages/python/scripts packages/python/tests
uv run --project packages/python --locked --group dev \
    ruff format --check packages/python/termtrace packages/python/scripts packages/python/tests
