#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
export TERMTRACE_GHOSTTY_PREFIX="${TERMTRACE_GHOSTTY_PREFIX:-$root/vendor/ghostty/zig-out}"
readonly crates=(model terminal evidence engine render media export service scenario sdk)
mkdir -p "$root/target"
readonly stage="$(mktemp -d "$root/target/rust-packages.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
package_args=()
for crate in "${crates[@]}"; do
    package_args+=(-p "termtrace-$crate")
done
# A fresh temporary registry prevents Cargo from reusing an earlier unpublished
# archive with the same version during iterative package validation.
cargo package --locked --target-dir "$stage" "${package_args[@]}" "$@"
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
mkdir -p dist/rust
for crate in "${crates[@]}"; do
    tar -xOf "$stage/package/termtrace-$crate-$version.crate" "termtrace-$crate-$version/LICENSE" | cmp LICENSE -
    cp "$stage/package/termtrace-$crate-$version.crate" dist/rust/
done
(cd dist/rust && sha256sum ./*.crate >packages.sha256)
