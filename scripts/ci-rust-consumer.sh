#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly consumer="$root/tests/consumers/rust/Cargo.toml"
readonly scratch="$(mktemp -d)"
export TERMTRACE_BIN="${TERMTRACE_BIN:-$root/target/debug/termtrace}"
recorder_pid=""
cleanup() {
    local result=$?
    if [[ "$result" -ne 0 ]] && [[ -f "$scratch/recorder.log" ]]; then
        cat "$scratch/recorder.log" >&2
    fi
    if [[ -n "$recorder_pid" ]] && kill -0 "$recorder_pid" 2>/dev/null; then
        kill -TERM "$recorder_pid"
        wait "$recorder_pid" || true
    fi
    rm -rf "$scratch"
}
trap cleanup EXIT

# This standalone workspace prevents the CLI's in-process feature from hiding
# an accidental engine/native dependency in the default public SDK.
cargo tree --locked --manifest-path "$consumer" --edges normal --prefix none >"$scratch/dependencies"
if rg -q '^termtrace-(engine|terminal|media|render|export|evidence) ' "$scratch/dependencies"; then
    cat "$scratch/dependencies"
    exit 1
fi
cargo build --locked --manifest-path "$consumer"
"${TERMTRACE_BIN:-$root/target/debug/termtrace}" record --no-input --out "$scratch/session" -- /bin/sh -c 'trap "exit 0" TERM; printf "consumer ready\n"; while :; do sleep 1; done' >"$scratch/recorder.log" 2>&1 &
recorder_pid=$!
cargo run --locked --quiet --manifest-path "$consumer" -- "$scratch/session"
wait "$recorder_pid"
recorder_pid=""
test -s "$scratch/session/completion.json"
