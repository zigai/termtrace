#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 RELEASE_BINARY WHEEL_PLATFORM_TAG" >&2
    exit 2
fi

binary=$(realpath "$1")
platform=$2
package_bin=packages/python/src/termtrace/bin
cleanup() {
    rm -rf "$package_bin"
}
trap cleanup EXIT
mkdir -p "$package_bin"
install -m 0555 "$binary" "$package_bin/termtrace"
"$package_bin/termtrace" doctor --require semantic
uv build --wheel packages/python --out-dir dist/python
wheel=$(find dist/python -maxdepth 1 -name '*.whl' -print -quit)
if [[ -z "$wheel" ]]; then
    echo "wheel build produced no artifact" >&2
    exit 1
fi
uvx --from wheel wheel tags --platform-tag "$platform" --remove "$wheel"
wheel=$(find dist/python -maxdepth 1 -name "*-${platform}.whl" -print -quit)
if [[ -z "$wheel" ]]; then
    echo "retagged wheel was not found for $platform" >&2
    exit 1
fi
uvx --from auditwheel auditwheel show "$wheel"
