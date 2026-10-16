#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${TERMTRACE_BINARY:-$root/target/debug/termtrace}"
[[ -x "$binary" ]] || {
    printf 'missing Termtrace binary: %s\n' "$binary" >&2
    exit 1
}
binary="$(realpath "$binary")"
for tool in ghostty Xvfb dbus-run-session import ffmpeg ffprobe jq timeout; do
    command -v "$tool" >/dev/null || {
        printf 'missing native test tool: %s\n' "$tool" >&2
        exit 1
    }
done
real_ghostty="$(command -v ghostty)"
real_xvfb="$(command -v Xvfb)"
real_dbus="$(command -v dbus-run-session)"
real_import="$(command -v import)"
real_ffmpeg="$(command -v ffmpeg)"
real_ffprobe="$(command -v ffprobe)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export TERMTRACE_PROBE_REAL_GHOSTTY="$real_ghostty" TERMTRACE_PROBE_REAL_XVFB="$real_xvfb"
export TERMTRACE_PROBE_REAL_DBUS="$real_dbus" TERMTRACE_PROBE_REAL_IMPORT="$real_import"
export TERMTRACE_PROBE_REAL_FFMPEG="$real_ffmpeg" TERMTRACE_PROBE_REAL_FFPROBE="$real_ffprobe"

prepare_case() {
    local directory="$1" broken="$2" tool variable
    mkdir -p "$directory/bin" "$directory/tmp"
    export TERMTRACE_PROBE_PIDS="$directory/pids" TERMTRACE_PROBE_DISPLAYS="$directory/displays"
    : >"$TERMTRACE_PROBE_PIDS"
    : >"$TERMTRACE_PROBE_DISPLAYS"
    export TERMTRACE_PROBE_CALLS="$directory/calls"
    : >"$TERMTRACE_PROBE_CALLS"
    for tool in ghostty Xvfb dbus-run-session import ffmpeg ffprobe; do
        case "$tool" in
            ghostty) variable=TERMTRACE_PROBE_REAL_GHOSTTY ;;
            Xvfb) variable=TERMTRACE_PROBE_REAL_XVFB ;;
            dbus-run-session) variable=TERMTRACE_PROBE_REAL_DBUS ;;
            import) variable=TERMTRACE_PROBE_REAL_IMPORT ;;
            ffmpeg) variable=TERMTRACE_PROBE_REAL_FFMPEG ;;
            ffprobe) variable=TERMTRACE_PROBE_REAL_FFPROBE ;;
        esac
        {
            printf '#!/bin/sh\nprintf "%%s\\n" "$$" >> "$TERMTRACE_PROBE_PIDS"\n'
            printf 'printf "%%s\\n" "%s" >> "$TERMTRACE_PROBE_CALLS"\n' "$tool"
            if [[ "$tool" == Xvfb && "$broken" != Xvfb ]]; then
                printf 'printf "%%s\\n" "$1" >> "$TERMTRACE_PROBE_DISPLAYS"\n'
            fi
            if [[ "$broken" == "$tool" ]]; then
                printf 'exec /usr/bin/false\n'
            elif [[ "$broken" == isolated-native && "$tool" == ghostty ]]; then
                printf 'exec /usr/bin/false\n'
            elif [[ "$broken" == isolated-native && "$tool" =~ ^(Xvfb|dbus-run-session|import)$ ]]; then
                printf 'exec /bin/sleep 60\n'
            elif [[ "$broken" == unpainted-ghostty && "$tool" == ghostty ]]; then
                printf 'for ready do :; done\nprintf ready > "$ready"\n/bin/sleep 60 &\nprintf "%%s\\n" "$!" >> "$TERMTRACE_PROBE_PIDS"\nwait\n'
            elif [[ "$broken" == noisy-ffmpeg && "$tool" == ffmpeg ]]; then
                printf 'while :; do printf "unbounded probe diagnostics 0123456789\\n" >&2; done\n'
            elif [[ "$broken" == noisy-ffprobe && "$tool" == ffprobe ]]; then
                printf 'while :; do printf "unbounded probe output 0123456789\\n"; done\n'
            elif [[ "$broken" == "hanging-$tool" ]]; then
                printf '/bin/sleep 60 &\nprintf "%%s\\n" "$!" >> "$TERMTRACE_PROBE_PIDS"\nwait\n'
            else
                printf 'exec "$%s" "$@"\n' "$variable"
            fi
        } >"$directory/bin/$tool"
        chmod +x "$directory/bin/$tool"
    done
}

# Every doctor case launches actual tools except the deliberately broken dependency.
# PID/display evidence belongs to this invocation, never another session.
run_case() {
    local name="$1" broken="$2" requirement="$3" expected="$4" code="$5"
    local directory="$scratch/$name" status=0 pid display state path
    prepare_case "$directory" "$broken"
    PATH="$directory/bin:$PATH" TMPDIR="$directory/tmp" timeout --kill-after=2 45 \
        "$binary" doctor --require "$requirement" --json >"$directory/report.json" || status=$?
    [[ "$status" == "$expected" ]] || {
        printf '%s: expected exit %s, got %s\n' "$name" "$expected" "$status" >&2
        cat "$directory/report.json" >&2
        return 1
    }
    jq -e --arg id "capabilities.$requirement" --arg code "$code" \
        '.checks[] | select(.id == $id) | .code == $code' "$directory/report.json" >/dev/null
    jq -e '.checks[] | select(.id == "capabilities.semantic") | .status == "ok"' "$directory/report.json" >/dev/null
    if [[ "$broken" == import ]]; then
        jq -e '.checks[] | select(.id == "capabilities.native-video") | .status == "ok"' "$directory/report.json" >/dev/null
    elif [[ "$broken" == ffmpeg || "$broken" == hanging-ffmpeg ]]; then
        jq -e '.checks[] | select(.id == "capabilities.native-screenshot") | .status == "ok"' "$directory/report.json" >/dev/null
    fi
    if [[ "$broken" != Xvfb ]]; then
        [[ "$(wc -l <"$TERMTRACE_PROBE_DISPLAYS")" == 1 ]] || {
            printf '%s: screenshot/video did not share one display probe\n' "$name" >&2
            return 1
        }
    fi
    while read -r pid; do
        if [[ -r "/proc/$pid/stat" ]]; then
            state="$(sed -E 's/^.*\) ([A-Z]).*/\1/' "/proc/$pid/stat")"
            [[ "$state" == Z ]] || {
                printf '%s: leaked live child %s\n' "$name" "$pid" >&2
                return 1
            }
        fi
    done <"$TERMTRACE_PROBE_PIDS"
    while read -r display; do
        display="${display#:}"
        for path in "/tmp/.X$display-lock" "/tmp/.X11-unix/X$display"; do
            [[ ! -e "$path" ]] || {
                printf '%s: leaked display resource %s\n' "$name" "$path" >&2
                return 1
            }
        done
    done <"$TERMTRACE_PROBE_DISPLAYS"
    for path in "$directory/tmp"/termtrace-capability-*; do
        [[ ! -e "$path" ]] || {
            printf '%s: leaked probe directory\n' "$name" >&2
            return 1
        }
    done
    printf '%s: passed\n' "$name"
}

assert_not_invoked() {
    local forbidden="$1"
    if grep -Ex "$forbidden" "$TERMTRACE_PROBE_CALLS"; then
        printf 'unrelated capability tools were invoked: %s\n' "$forbidden" >&2
        return 1
    fi
}

run_isolation() {
    local directory="$scratch/isolation" session="$scratch/isolation.termtrace"
    timeout --kill-after=2 60 "$binary" record --no-input --geometry fit --viewport 640x400 \
        --out "$session" -- /bin/sh -c \
        'printf "\033[2J\033[Hcapability isolation\n"; /bin/sleep 0.2; printf "complete\n"'
    "$binary" analyze index --session "$session"

    prepare_case "$directory/video" hanging-import
    PATH="$directory/video/bin:$PATH" timeout --kill-after=2 90 \
        "$binary" artifact capture --session "$session" --fps 24
    assert_not_invoked import
    jq -e '.capture_kind == "video" and .video_probe.streams[0].codec_name == "h264"' \
        "$session/visual/manifest.json" >/dev/null

    prepare_case "$directory/screenshot" hanging-ffmpeg
    PATH="$directory/screenshot/bin:$PATH" timeout --kill-after=2 90 \
        "$binary" screenshot "$session" --renderer native --out "$directory/native-screenshot"
    assert_not_invoked ffmpeg
    jq -e '.capture_kind == "screenshot" and .screenshot_probe.streams[0].codec_name == "png"' \
        "$directory/native-screenshot/manifest.json" >/dev/null

    prepare_case "$directory/export" isolated-native
    PATH="$directory/export/bin:$PATH" timeout --kill-after=2 30 \
        "$binary" screenshot "$session" --renderer semantic --out "$directory/semantic-screenshot"
    assert_not_invoked 'ghostty|Xvfb|dbus-run-session|import|ffmpeg|ffprobe'
    local renderer
    for renderer in semantic native; do
        PATH="$directory/export/bin:$PATH" timeout --kill-after=2 90 \
            "$binary" video "$session" --renderer "$renderer" --fps 24 \
            --hold-start 0ms --hold-end 0ms --out "$directory/$renderer.mp4"
        assert_not_invoked 'ghostty|Xvfb|dbus-run-session|import'
        "$real_ffprobe" -v error -show_streams -of json "$directory/$renderer.mp4" |
            jq -e '.streams[0].codec_name == "h264" and (.streams[0].duration | tonumber) > 0' >/dev/null
    done
    printf 'targeted native capture and semantic/retained-native export isolation: passed\n'
}

run_isolation

run_case healthy-screenshot '' native-screenshot 0 native_screenshot_ready
run_case healthy-video '' native-video 0 native_video_ready
run_case broken-ghostty ghostty native-screenshot 1 native_screenshot_probe_failed
run_case broken-display Xvfb native-screenshot 1 native_screenshot_probe_failed
run_case broken-screenshot import native-screenshot 1 native_screenshot_probe_failed
run_case broken-codec ffmpeg native-video 1 native_video_probe_failed
run_case broken-media-probe ffprobe native-screenshot 1 native_screenshot_probe_failed
run_case hanging-ghostty hanging-ghostty native-screenshot 1 native_screenshot_probe_failed
run_case hanging-codec hanging-ffmpeg native-video 1 native_video_probe_failed
run_case hanging-media-probe hanging-ffprobe native-screenshot 1 native_screenshot_probe_failed
run_case unpainted-root unpainted-ghostty native-screenshot 1 native_screenshot_probe_failed
run_case noisy-codec noisy-ffmpeg native-video 1 native_video_probe_failed
run_case noisy-media-probe noisy-ffprobe native-screenshot 1 native_screenshot_probe_failed
