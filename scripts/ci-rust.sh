#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

export TERMTRACE_GHOSTTY_PREFIX="${TERMTRACE_GHOSTTY_PREFIX:-$root/vendor/ghostty/zig-out}"

for file in install.sh benchmarks/*.sh scripts/*.sh examples/*.sh; do
    bash -n "$file"
done
bash tests/install.sh

python3 -m py_compile examples/apps/terminal_showcase.py
python3 scripts/check-architecture.py
python3 -m unittest discover -s scripts -p test_check_architecture.py

scripts/bootstrap.sh
bash scripts/check-github-action-pins.sh

cargo machete
cargo fmt --all -- --check
cargo statement-spacing check
cargo fmt --manifest-path tests/consumers/rust/Cargo.toml -- --check
cargo test --locked -p termtrace-sdk --no-default-features
cargo clippy --locked -p termtrace-sdk --no-default-features --all-targets -- -D warnings
cargo test --locked --workspace --lib --bins --test execution_lifecycle
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
