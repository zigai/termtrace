#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

uv run --project "$root/packages/python" --group codegen \
  python "$root/packages/python/scripts/generate_protocol.py"
bun run --cwd "$root/packages/typescript" generate:protocol
