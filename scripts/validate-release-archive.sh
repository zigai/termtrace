#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 RELEASE_ARCHIVE.tar.gz|RELEASE_ARCHIVE.tar.zst" >&2
    exit 2
fi

archive=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$archive" in
    *.tar.gz | *.tgz) tar -xzf "$archive" -C "$work" ;;
    *.tar.zst) tar --zstd -xf "$archive" -C "$work" ;;
    *)
        echo "unsupported release archive: $archive" >&2
        exit 2
        ;;
esac

binary=$(find "$work" -type f -name termtrace -perm -u+x -print -quit)
if [[ -z "$binary" ]]; then
    echo "release archive contains no executable termtrace binary" >&2
    exit 1
fi

archive_root=$(dirname "$binary")
test -f "$archive_root/NOTICE"

"$binary" version --json | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["termtrace_version"] and value["build_revision"]'
"$binary" doctor --require core,semantic

session="$work/install-check.termtrace"
"$binary" record --no-input --out "$session" -- /bin/sh -c 'printf RELEASE-CHECK'
"$binary" integrity verify "$session" --level content --json >/dev/null
test "$("$binary" show "$session" --format text | tr -d ' ')" = "RELEASE-CHECK"
