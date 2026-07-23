#!/usr/bin/env bash
set -Eeuo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly binary="${TERMTRACE_BINARY:-$root/target/release/termtrace}"
readonly warmups="${RENDER_WARMUPS:-1}"
readonly repetitions="${RENDER_REPETITIONS:-3}"
readonly output="${OUTPUT:-$root/benchmarks/results/render.jsonl}"
readonly work="$(mktemp -d)"

cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

if [[ ! "$warmups" =~ ^[0-9]+$ || ! "$repetitions" =~ ^[1-9][0-9]*$ ]]; then
    printf 'RENDER_WARMUPS and RENDER_REPETITIONS must be nonnegative/positive integers\n' >&2
    exit 2
fi
if [[ ! -x "$binary" ]]; then
    printf 'Termtrace binary is not executable: %s\n' "$binary" >&2
    exit 2
fi

mkdir -p "$(dirname "$output")"
: >"$output"
session="$work/session.termtrace"
edit="$work/edit.json"

"$binary" record \
    --no-input \
    --cols 120 \
    --rows 40 \
    --cell-width 10 \
    --cell-height 20 \
    --out "$session" \
    -- /bin/sh -c '
        i=0
        while [ "$i" -lt 120 ]; do
            printf "\033[Hframe %04d\033[K\n" "$i"
            i=$((i + 1))
        done
    ' >/dev/null

end_ns=$(jq -s -r '.[-1].monotonic_ns' "$session/events.jsonl")
jq -n --argjson end_ns "$end_ns" '{
    version: 1,
    clips: [{from: {monotonicNs: 0}, to: {monotonicNs: $end_ns}, caption: null}],
    change_visualization: null
}' >"$edit"

measure() {
    local operation=$1
    local iteration=$2
    local record_result=${3:-true}
    local destination="$work/$operation-$iteration"
    local started_ns ended_ns elapsed_ns artifact_hash
    started_ns=$(date +%s%N)
    case "$operation" in
        semantic-video-export)
            "$binary" artifact video export "$session" \
                --source semantic \
                --fps 24 \
                --edit "$edit" \
                --out "$destination" >/dev/null
            artifact_hash=$(jq -r '.output_video_hash' "$destination/manifest.json")
            ;;
        presentation-preview)
            "$binary" artifact video preview "$session" \
                --source semantic \
                --edit "$edit" \
                --out "$destination" >/dev/null
            artifact_hash=$(jq -r '.members[] | select(.path == "preview.mp4") | .blake3' "$destination/manifest.json")
            ;;
        *)
            printf 'unsupported render operation: %s\n' "$operation" >&2
            exit 2
            ;;
    esac
    ended_ns=$(date +%s%N)
    elapsed_ns=$((ended_ns - started_ns))
    if [[ "$record_result" == true ]]; then
        jq -cn \
            --arg operation "$operation" \
            --argjson iteration "$iteration" \
            --argjson elapsed_ns "$elapsed_ns" \
            --arg artifact_hash "$artifact_hash" \
            '{operation:$operation,iteration:$iteration,elapsed_ns:$elapsed_ns,artifact_hash:$artifact_hash}' \
            >>"$output"
    fi
}

for operation in semantic-video-export presentation-preview; do
    for ((iteration = 1; iteration <= warmups; iteration += 1)); do
        measure "$operation" "$iteration" false
    done
    for ((iteration = 1; iteration <= repetitions; iteration += 1)); do
        measure "$operation" "$iteration" true
    done
done

jq -s -e \
    --argjson repetitions "$repetitions" \
    'length == (2 * $repetitions) and
     all(.[]; .artifact_hash | strings | length == 64)' \
    "$output" >/dev/null
printf '%s\n' "$output"
