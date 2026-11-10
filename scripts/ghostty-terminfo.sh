#!/usr/bin/env bash
# Write the pinned Ghostty terminfo entry into crates/terminal/terminfo, or with --check fail
# when the committed source no longer matches the pinned Ghostty.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly zig="${TERMTRACE_ZIG:?TERMTRACE_ZIG must name the zig executable}"
readonly destination="$root/crates/terminal/terminfo"
readonly cache="$root/.cache/ghostty-terminfo"

mkdir -p "$cache"
source="$cache/ghostty.terminfo"
"$zig" run \
    --cache-dir "$cache/zig-cache" \
    --global-cache-dir "$cache/zig-global-cache" \
    --dep terminfo \
    -Mroot="$root/scripts/ghostty-terminfo/main.zig" \
    -Mterminfo="$root/vendor/ghostty/src/terminfo/ghostty.zig" \
    >"$source"

if [[ "${1:-}" == --check ]]; then
    if ! cmp --quiet "$source" "$destination/ghostty.terminfo"; then
        echo "crates/terminal/terminfo is stale for the pinned Ghostty; run TERMTRACE_ZIG=$zig scripts/ghostty-terminfo.sh" >&2
        exit 1
    fi
    exit 0
fi

compiled="$cache/compiled"
rm -rf "$compiled"
mkdir -p "$compiled"
tic -x -o "$compiled" "$source" 2>/dev/null
cp "$source" "$destination/ghostty.terminfo"
cp "$compiled/x/xterm-ghostty" "$destination/xterm-ghostty"
