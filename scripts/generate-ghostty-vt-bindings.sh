#!/usr/bin/env bash
set -euo pipefail

readonly bindgen_version=0.72.1
readonly ghostty_revision=55a3e33ab26a23d75b274b23c7f76d837db00578
readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ghostty="$root/vendor/ghostty"
readonly header="$ghostty/include/ghostty/vt.h"
readonly output="$root/crates/terminal/src/ghostty_vt/bindings.rs"
temporary_output=""

cleanup() {
    if [[ -n "$temporary_output" ]]; then
        rm -f "$temporary_output"
    fi
}

trap cleanup EXIT

if [[ ! -f "$header" ]]; then
    echo "Ghostty submodule is missing; run scripts/bootstrap.sh" >&2
    exit 2
fi

if [[ "$(git -C "$ghostty" rev-parse HEAD)" != "$ghostty_revision" ]]; then
    echo "refusing to generate bindings from an unpinned Ghostty revision" >&2
    exit 2
fi

if ! command -v bindgen >/dev/null 2>&1; then
    echo "bindgen is required only to upgrade bindings" >&2
    echo "install it with: cargo install bindgen-cli --version $bindgen_version --locked" >&2
    exit 2
fi

if [[ "$(bindgen --version)" != "bindgen $bindgen_version" ]]; then
    echo "bindgen $bindgen_version is required" >&2
    exit 2
fi

temporary_output="$(mktemp "${output}.XXXXXX")"
bindgen "$header" \
    --allowlist-function 'ghostty_.*' \
    --allowlist-type 'Ghostty.*' \
    --allowlist-var 'GHOSTTY_.*' \
    --with-derive-default \
    --no-layout-tests \
    --output "$temporary_output" \
    -- \
    "-I$ghostty/include"

chmod 0644 "$temporary_output"
mv "$temporary_output" "$output"
temporary_output=""

printf 'updated %s from Ghostty %s\n' "$output" "$ghostty_revision"
