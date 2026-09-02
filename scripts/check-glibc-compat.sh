#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 BINARY MAX_GLIBC_VERSION" >&2
    exit 2
fi

binary=$1
maximum=$2

if [[ ! -x "$binary" ]]; then
    echo "binary is not executable: $binary" >&2
    exit 2
fi

if ! command -v objdump >/dev/null 2>&1; then
    echo "objdump is required to inspect GLIBC symbols" >&2
    exit 2
fi

required=$(
    objdump -T "$binary" |
        sed -n 's/.*GLIBC_\([0-9][0-9.]*\).*/\1/p' |
        sort -Vu |
        tail -n 1
)
if [[ -z "$required" ]]; then
    echo "no GLIBC symbol versions found in $binary" >&2
    exit 1
fi

highest=$(printf '%s\n%s\n' "$required" "$maximum" | sort -Vu | tail -n 1)
if [[ "$highest" != "$maximum" ]]; then
    echo "$binary requires GLIBC $required, newer than supported maximum $maximum" >&2
    exit 1
fi

printf '%s requires GLIBC %s or older (maximum %s)\n' "$binary" "$required" "$maximum"
