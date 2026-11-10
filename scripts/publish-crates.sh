#!/usr/bin/env bash
# Publish every workspace crate missing from crates.io. crates.io limits how fast new crates can
# be created, so a rejected run waits and publishes the rest.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
mapfile -t packages < <(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.publish == null) | .name')
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')

published() {
    local name=$1
    curl --fail --silent --show-error "https://index.crates.io/${name:0:2}/${name:2:2}/$name" 2>/dev/null |
        jq -e --arg version "$version" 'select(.vers == $version)' >/dev/null
}

for attempt in $(seq 1 16); do
    exclude=()
    remaining=0
    for package in "${packages[@]}"; do
        if published "$package"; then
            exclude+=(--exclude "$package")
        else
            remaining=$((remaining + 1))
        fi
    done

    if [[ "$remaining" -eq 0 ]]; then
        exit 0
    fi

    echo "attempt $attempt: publishing $remaining crates"
    if cargo publish --locked --workspace "${exclude[@]}"; then
        exit 0
    fi

    sleep 600
done

echo "crates are still unpublished after $attempt attempts" >&2
exit 1
