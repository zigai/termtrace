#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for file in install.sh benchmarks/*.sh scripts/*.sh examples/*.sh; do
    bash -n "$file"
done
bash tests/install.sh

python3 -m py_compile examples/apps/terminal_showcase.py
python3 scripts/check-architecture.py

scripts/bootstrap.sh
bash scripts/check-github-action-pins.sh

cargo machete
cargo fmt --all -- --check
cargo test --locked --workspace --lib --bins
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
