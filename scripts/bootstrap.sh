#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$(uname -s)" != Linux ]]; then
    echo "Termtrace source builds currently support Linux only" >&2
    exit 2
fi

if [[ -e "$root/.git" ]]; then
    git -C "$root" submodule update --init --recursive -- vendor/ghostty
elif [[ ! -f "$root/vendor/ghostty/build.zig" ]]; then
    echo "Ghostty sources are missing and this is not a Git checkout" >&2
    exit 2
fi

"$root/scripts/bootstrap-ghostty-vt.sh"
