#!/usr/bin/env bash

set -Eeuo pipefail

DEMO_DIRECTORY=$(cd "$(dirname "$0")" && pwd)
OUTPUT_ROOT=${1:-"$(cd "$DEMO_DIRECTORY/.." && pwd)/demo-artifacts"}

"$DEMO_DIRECTORY/demo.sh" "$OUTPUT_ROOT"

printf '\nTermtrace demo completed: %s\n' "$OUTPUT_ROOT/showcase"
