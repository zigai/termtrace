#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bash -n install.sh benchmarks/*.sh scripts/*.sh demos/*.sh
bash tests/install.sh
python3 -m py_compile demos/apps/terminal_showcase.py
scripts/bootstrap.sh
bash scripts/check-github-action-pins.sh
python3 scripts/check-architecture.py
cargo machete
cargo fmt --all -- --check
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
