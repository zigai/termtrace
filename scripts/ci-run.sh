#!/usr/bin/env bash
set -euo pipefail
umask 077

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly label="${1:?usage: ci-run.sh LABEL COMMAND [ARG ...]}"
shift
[[ "$label" =~ ^[a-zA-Z0-9_-]+$ ]] || {
    echo 'invalid CI evidence label' >&2
    exit 2
}
(($#)) || {
    echo 'missing CI command' >&2
    exit 2
}
cd "$root"
if [[ -f "$root/.tools/ci-env.sh" ]]; then
    source "$root/.tools/ci-env.sh"
fi
# Keep Unix socket paths short regardless of the checkout location. Artifact
# collection follows this per-invocation link; no other /tmp content is included.
readonly evidence_root="$(mktemp -d /tmp/ttci.XXXXXX)"
mkdir -p "$root/target/ci-evidence"
ln -s "$evidence_root" "$root/target/ci-evidence/$label-${evidence_root##*/}"
export TERMTRACE_CI_EVIDENCE_ROOT="$evidence_root"
export TMPDIR="$TERMTRACE_CI_EVIDENCE_ROOT/tmp"
export XDG_RUNTIME_DIR="$TERMTRACE_CI_EVIDENCE_ROOT/runtime"
export RUST_BACKTRACE=1
mkdir -p "$TMPDIR" "$XDG_RUNTIME_DIR"
{
    printf 'command:'
    printf ' %q' "$@"
    printf '\nTERM=%s\nLANG=%s\n' "${TERM:-}" "${LANG:-}"
    "$@"
} 2>&1 | tee "$TERMTRACE_CI_EVIDENCE_ROOT/command.log"
