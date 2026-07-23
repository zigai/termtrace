#!/usr/bin/env bash

set -Eeuo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly repetitions="${REPETITIONS:-30}"
readonly payload_bytes="${PAYLOAD_BYTES:-1048576}"
readonly output="${OUTPUT:-$root/benchmarks/results/durability.jsonl}"
readonly sessions="$(mktemp -d)"

cleanup() {
    rm -rf "$sessions"
}
trap cleanup EXIT

if ! [[ "$repetitions" =~ ^[1-9][0-9]*$ && "$payload_bytes" =~ ^[1-9][0-9]*$ ]]; then
    printf 'REPETITIONS and PAYLOAD_BYTES must be positive integers\n' >&2
    exit 2
fi

mkdir -p "$(dirname "$output")"
: >"$output"

cd "$root"
cargo build --release --all-features

for mode in strict bounded throughput; do
    for ((iteration = 1; iteration <= repetitions; iteration += 1)); do
        session="$sessions/$mode-$iteration.termtrace"
        started_ns="$(date +%s%N)"
        target/release/termtrace record --no-input --durability "$mode" \
            --out "$session" -- /bin/sh -c 'head -c "$1" /dev/zero' sh "$payload_bytes" \
            >/dev/null &
        recorder_pid="$!"
        maximum_rss_kib=0
        while kill -0 "$recorder_pid" 2>/dev/null; do
            if [[ -r "/proc/$recorder_pid/status" ]]; then
                current_rss_kib="$(awk '/^VmHWM:/ { print $2 }' "/proc/$recorder_pid/status" 2>/dev/null || true)"
                if [[ -n "$current_rss_kib" && "$current_rss_kib" -gt "$maximum_rss_kib" ]]; then
                    maximum_rss_kib="$current_rss_kib"
                fi
            fi
            sleep 0.01
        done
        wait "$recorder_pid"
        elapsed_ns="$(($(date +%s%N) - started_ns))"
        jq -cn \
            --arg mode "$mode" \
            --argjson iteration "$iteration" \
            --argjson payload_bytes "$payload_bytes" \
            --argjson elapsed_ns "$elapsed_ns" \
            --argjson maximum_rss_kib "$maximum_rss_kib" \
            --slurpfile completion "$session/completion.json" \
            '{
        mode: $mode,
        iteration: $iteration,
        payload_bytes: $payload_bytes,
        elapsed_ns: $elapsed_ns,
        maximum_rss_bytes: ($maximum_rss_kib * 1024),
        durability: $completion[0].durability
      }' >>"$output"
        rm -rf "$session"
    done
done

jq -s -e \
    --argjson repetitions "$repetitions" \
    'length == (3 * $repetitions) and all(.[]; .durability.final_sync_completed)' \
    "$output" >/dev/null
printf '%s\n' "$output"
