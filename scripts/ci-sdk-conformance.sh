#!/usr/bin/env bash
# Build the CLI and both native bindings, then certify every SDK capability row.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo build --locked
uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml
bunx @napi-rs/cli build --cwd crates/bindings-node
python3 scripts/sdk-conformance.py --profile certify "$@"
