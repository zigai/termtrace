#!/usr/bin/env bash
# Build the wheel for one target with the release CLI bundled beside the extension.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <rust-target> <termtrace-binary>" >&2
    exit 2
fi

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly target=$1
readonly cli=$2
readonly bundled="$root/crates/bindings-python/python/termtrace/termtrace"

install -m 0755 "$cli" "$bundled"
trap 'rm -f "$bundled"' EXIT
uvx --from maturin==1.16.0 maturin build --release --locked --compatibility pypi --target "$target" --manifest-path "$root/crates/bindings-python/Cargo.toml" --out "$root/dist/python"
