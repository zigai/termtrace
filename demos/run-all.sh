#!/usr/bin/env bash

set -Eeuo pipefail

DEMO_DIRECTORY=$(cd "$(dirname "$0")" && pwd)
OUTPUT_ROOT=${1:-"$(cd "$DEMO_DIRECTORY/.." && pwd)/demo-artifacts"}

"$DEMO_DIRECTORY/01-visual-semantics.sh" "$OUTPUT_ROOT"
"$DEMO_DIRECTORY/02-control-lifecycle.sh" "$OUTPUT_ROOT"
"$DEMO_DIRECTORY/03-tmux-recovery.sh" "$OUTPUT_ROOT"

printf '\nAll Termtrace demos completed: %s\n' "$OUTPUT_ROOT"
