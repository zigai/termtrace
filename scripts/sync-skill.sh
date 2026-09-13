#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SOURCE_DIR="$REPO_ROOT/skills/termtrace"

# Target path in local skills catalog repository
TARGET_DIR="${SKILLS_CATALOG_DIR:-"$REPO_ROOT/../skills/catalog/global/termtrace"}"

if [ -d "$TARGET_DIR" ] || [ -d "$(dirname "$TARGET_DIR")" ]; then
    mkdir -p "$TARGET_DIR"
    rsync -a --delete "$SOURCE_DIR/" "$TARGET_DIR/"
    echo "Synchronized termtrace skill: $SOURCE_DIR -> $TARGET_DIR"
fi
