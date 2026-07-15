#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 RELEASE_ARCHIVE.tar.zst" >&2
    exit 2
fi

archive=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar --zstd -xf "$archive" -C "$work"
binary=$(find "$work" -type f -name termtrace -perm -u+x -print -quit)
if [[ -z "$binary" ]]; then
    echo "release archive contains no executable termtrace binary" >&2
    exit 1
fi

"$binary" version --json | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["termtrace_version"] and value["build_revision"]'
"$binary" doctor --require core,semantic
session="$work/install-smoke.termtrace"
"$binary" record --no-input --out "$session" -- /bin/sh -c 'printf RELEASE-SMOKE'
"$binary" verify "$session" --level content --json >/dev/null
test "$("$binary" show "$session" --format text | tr -d ' ')" = "RELEASE-SMOKE"
