#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly workflow_root="$root/.github/workflows"
status=0

while IFS=: read -r file line definition; do
    location="$file:$line"
    value="${definition#*uses:}"
    value="${value%%#*}"
    value="${value//[[:space:]]/}"

    if [[ "$value" == ./* || "$value" =~ ^[^@]+@[0-9a-f]{40}$ ]]; then
        continue
    fi

    printf 'mutable GitHub Action reference: %s (%s)\n' "$value" "$location" >&2
    status=1
done < <(
    rg --line-number --no-heading \
        --glob '*.yml' --glob '*.yaml' \
        '^[[:space:]]*(-[[:space:]]*)?uses:' \
        "$workflow_root"
)

exit "$status"
