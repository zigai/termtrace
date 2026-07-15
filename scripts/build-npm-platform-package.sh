#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 RELEASE_BINARY linux x64|arm64" >&2
    exit 2
fi

binary=$(realpath "$1")
os=$2
arch=$3
case "$os/$arch" in
    linux/x64|linux/arm64) ;;
    *) echo "unsupported npm platform: $os/$arch" >&2; exit 2 ;;
esac
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
staging="packages/npm-staging/cli-$os-$arch"
cleanup() {
    rm -rf "$staging"
}
trap cleanup EXIT
rm -rf "$staging"
mkdir -p "$staging/bin" dist/npm
install -m 0555 "$binary" "$staging/bin/termtrace"
"$staging/bin/termtrace" doctor --require semantic
python3 - "$staging/package.json" "$version" "$os" "$arch" <<'PY'
import json
import pathlib
import sys

path, version, os_name, arch = sys.argv[1:]
manifest = {
    "name": f"@termtrace/cli-{os_name}-{arch}",
    "version": version,
    "description": "Platform binary for @termtrace/test",
    "os": [os_name],
    "cpu": [arch],
    "bin": {"termtrace": "bin/termtrace"},
    "files": ["bin/termtrace"],
    "license": "MIT",
}
pathlib.Path(path).write_text(json.dumps(manifest, indent=2) + "\n")
PY
(cd "$staging" && npm pack --pack-destination ../../../dist/npm)
