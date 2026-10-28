#!/usr/bin/env bash

set -Eeuo pipefail

DEMO_REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DEMO_BUILD_RELEASE=
if [[ -z ${TERMTRACE_BIN:-} ]]; then
    TERMTRACE_BIN=$DEMO_REPO_ROOT/target/release/termtrace
    DEMO_BUILD_RELEASE=1
fi

require_demo_tools() {
    local tool
    for tool in "$@"; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            printf 'required demo tool is unavailable: %s\n' "$tool" >&2
            return 1
        fi
    done
}

prepare_demo_directory() {
    local directory=$1
    if [[ -e $directory ]]; then
        printf 'demo output already exists: %s\n' "$directory" >&2
        printf 'remove it or pass a different output root\n' >&2
        return 1
    fi
    mkdir -m 700 -p "$directory"
}

stop_session_on_error() {
    local session=$1
    if "$TERMTRACE_BIN" status "$session" --json 2>/dev/null | jq -e '.lifecycle == "live"' >/dev/null; then
        "$TERMTRACE_BIN" stop "$session" --wait >/dev/null 2>&1 || true
    fi
}

wait_for_published_capture() {
    local session=$1
    local state
    for _ in $(seq 1 600); do
        state=$("$TERMTRACE_BIN" status "$session" --json | jq -r '.original_capture.state')
        case $state in
            published) return 0 ;;
            live | finalizing) sleep 0.1 ;;
            *)
                printf 'original capture ended as %s\n' "$state" >&2
                return 1
                ;;
        esac
    done
    printf 'timed out waiting for the original capture: %s\n' "$session" >&2
    return 1
}

export_edited_videos() {
    local session=$1
    local demo_directory=$2
    local edit=$demo_directory/edit.json
    local extension
    jq -n '{
        version: 1,
        clips: [{
            from: {marker: "showcase-ready"},
            to: {marker: "signals-observed"},
            speed: {numerator: 1, denominator: 1},
            hold_ms: 500,
            caption: "State-guarded input and terminal signals"
        }],
        layout: {caption: "footer"}
    }' >"$edit"

    "$TERMTRACE_BIN" video "$session" --renderer original-execution --fps 24 --edit "$edit" \
        --preview-out "$demo_directory/preview" --json >"$demo_directory/preview.json"
    for extension in mp4 gif webm; do
        "$TERMTRACE_BIN" video "$session" --renderer original-execution --fps 24 --edit "$edit" \
            --out "$demo_directory/showcase.$extension" --json >"$demo_directory/video-$extension.json"
        ffprobe -v error -count_frames -select_streams v:0 \
            -show_entries stream=codec_name,width,height,nb_read_frames -show_entries format=duration \
            -of json "$demo_directory/showcase.$extension" >"$demo_directory/probe-$extension.json"
    done
}

print_demo_result() {
    local name=$1
    local directory=$2
    local session=$3
    printf '\n%s demo complete\n' "$name"
    printf '  artifacts:  %s\n' "$directory"
    printf '  session:    %s\n' "$session"
    printf '  videos:     %s\n' "$directory"/showcase.{mp4,gif,webm}
    printf '  preview:    %s\n' "$directory/preview/contact-sheet.png"
}
