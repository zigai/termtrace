#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
export TERMTRACE_GHOSTTY_PREFIX="${TERMTRACE_GHOSTTY_PREFIX:-$root/vendor/ghostty/zig-out}"
mapfile -t packages < <(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.publish == null) | .name')
mkdir -p "$root/target"
readonly stage="$(mktemp -d "$root/target/rust-packages.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
package_args=()
for package in "${packages[@]}"; do
    package_args+=(-p "$package")
done
# A fresh temporary registry prevents Cargo from reusing an earlier unpublished
# archive with the same version during iterative package validation.
cargo package --locked --target-dir "$stage" "${package_args[@]}" "$@"
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
mkdir -p dist/rust
for package in "${packages[@]}"; do
    tar -xOf "$stage/package/$package-$version.crate" "$package-$version/LICENSE" | cmp LICENSE -
    cp "$stage/package/$package-$version.crate" dist/rust/
done
(cd dist/rust && sha256sum ./*.crate >packages.sha256)
