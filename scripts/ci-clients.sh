#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bun install --cwd packages/typescript --frozen-lockfile
scripts/generate-client-protocol.sh
git diff --exit-code -- \
    packages/python/src/termtrace/_generated \
    packages/typescript/src/generated
bun test --cwd packages/typescript
node --check packages/typescript/bin/termtrace.js
bun run --cwd packages/typescript check
uv run --project packages/python --locked --group dev \
    pytest -q packages/python/tests
uv run --project packages/python --locked --group dev \
    mypy packages/python/src packages/python/tests
uv run --project packages/python --locked --group dev \
    ruff check --force-exclude \
    packages/python/src/termtrace packages/python/scripts packages/python/tests \
    --exclude packages/python/src/termtrace/_generated
uv run --project packages/python --locked --group dev \
    ruff format --check --force-exclude \
    packages/python/src/termtrace packages/python/scripts packages/python/tests \
    --exclude packages/python/src/termtrace/_generated
