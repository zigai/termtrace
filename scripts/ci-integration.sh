#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
bun install --cwd packages/typescript --frozen-lockfile
uv sync --project packages/python --locked --group dev
# Managed Python runners execute python3 directly; use the locked environment
# for those consumers as well as the tests that explicitly invoke uv.
export PATH="$root/packages/python/.venv/bin:$PATH"
cargo test --locked --test cli
