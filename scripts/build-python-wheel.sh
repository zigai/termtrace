#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 RELEASE_BINARY WHEEL_PLATFORM_TAG" >&2
    exit 2
fi

binary=$(realpath "$1")
platform=$2
package_bin=packages/python/termtrace/bin
wheel_stage=$(mktemp -d)

cleanup() {
    rm -rf "$package_bin"
    rm -rf "$wheel_stage"
}
trap cleanup EXIT

mkdir -p "$package_bin"
install -m 0555 "$binary" "$package_bin/termtrace"

"$package_bin/termtrace" doctor --require semantic

uv build --wheel packages/python --out-dir "$wheel_stage"

wheel=$(find "$wheel_stage" -maxdepth 1 -name '*.whl' -print -quit)
if [[ -z "$wheel" ]]; then
    echo "wheel build produced no artifact" >&2
    exit 1
fi

uvx --from wheel wheel tags --platform-tag "$platform" --remove "$wheel"

wheel=$(find "$wheel_stage" -maxdepth 1 -name "*-${platform}.whl" -print -quit)
if [[ -z "$wheel" ]]; then
    echo "retagged wheel was not found for $platform" >&2
    exit 1
fi

uvx --from auditwheel auditwheel show "$wheel"
mkdir -p dist/python
install -m 0444 "$wheel" dist/python/
