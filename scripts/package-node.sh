#!/usr/bin/env bash
# Pack the npm platform package for one target, or the main package with `main`.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly package_dir="$root/crates/bindings-node"
readonly out="$root/dist/npm"
mkdir -p "$out" "$root/target"
readonly stage="$(mktemp -d "$root/target/npm-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

pack_main() {
    node -e 'for (const file of require(process.argv[1]).files) console.log(file)' "$package_dir/package.json" | while read -r file; do
        install -m 0644 "$package_dir/$file" "$stage/$file"
    done
    install -m 0644 "$package_dir/package.json" "$root/LICENSE" "$stage/"
    local version
    version=$(node -p 'require(process.argv[1]).version' "$package_dir/package.json")
    for platform in "$package_dir"/npm/*/; do
        local name
        name=$(node -p 'require(process.argv[1]).name' "$platform/package.json")
        npm pkg set --prefix "$stage" "optionalDependencies.$name=$version"
    done
    npm pack --pack-destination "$out" "$stage"
}

pack_platform() {
    local target=$1 cli=$2 platform
    case "$target" in
        x86_64-unknown-linux-gnu) platform=linux-x64-gnu ;;
        aarch64-unknown-linux-gnu) platform=linux-arm64-gnu ;;
        *)
            echo "unsupported npm target: $target" >&2
            exit 2
            ;;
    esac
    cargo build --locked --release -p termtrace-node --lib --target "$target"
    install -m 0644 "$package_dir/npm/$platform/package.json" "$root/LICENSE" "$stage/"
    install -m 0755 "$root/target/$target/release/libtermtrace_node.so" "$stage/termtrace.$platform.node"
    install -m 0755 "$cli" "$stage/termtrace"
    npm pack --pack-destination "$out" "$stage"
}

case "${1:-}" in
    main) pack_main ;;
    *-linux-gnu)
        if [[ $# -ne 2 ]]; then
            echo "usage: $0 <rust-target> <termtrace-binary>" >&2
            exit 2
        fi
        pack_platform "$1" "$2"
        ;;
    *)
        echo "usage: $0 main | $0 <rust-target> <termtrace-binary>" >&2
        exit 2
        ;;
esac
