#!/usr/bin/env bash

set -Eeuo pipefail

DEMO_REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TERMTRACE_BIN=${TERMTRACE_BIN:-"$DEMO_REPO_ROOT/target/release/termtrace"}
RECORDER_PID=

require_demo_tools() {
  local tool
  for tool in "$@"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      printf 'required demo tool is unavailable: %s\n' "$tool" >&2
      return 1
    fi
  done
}

build_termtrace() {
  cargo build --manifest-path "$DEMO_REPO_ROOT/Cargo.toml" --locked --release
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

wait_for_control() {
  local session=$1
  local stderr_log=$2
  local attempt
  for attempt in $(seq 1 200); do
    if [[ -S $session/control.sock ]] && "$TERMTRACE_BIN" ping "$session" >/dev/null 2>&1; then
      return 0
    fi
    if [[ -n ${RECORDER_PID:-} ]] && ! kill -0 "$RECORDER_PID" 2>/dev/null; then
      printf 'recorder exited before control became ready\n' >&2
      tail -n 40 "$stderr_log" >&2 || true
      return 1
    fi
    sleep 0.05
  done
  printf 'timed out waiting for control socket: %s\n' "$session/control.sock" >&2
  tail -n 40 "$stderr_log" >&2 || true
  return 1
}

stop_recorder_on_error() {
  local session=${1:-}
  if [[ -n $session && -S $session/control.sock ]]; then
    "$TERMTRACE_BIN" stop "$session" >/dev/null 2>&1 || true
  fi
  if [[ -n ${RECORDER_PID:-} ]] && kill -0 "$RECORDER_PID" 2>/dev/null; then
    kill "$RECORDER_PID" 2>/dev/null || true
    wait "$RECORDER_PID" 2>/dev/null || true
  fi
}

wait_for_recorder() {
  local stderr_log=$1
  local status
  set +e
  wait "$RECORDER_PID"
  status=$?
  set -e
  RECORDER_PID=
  if ((status != 0)); then
    printf 'recorder failed with exit status %s\n' "$status" >&2
    tail -n 80 "$stderr_log" >&2 || true
    return "$status"
  fi
}

derive_and_capture() {
  local session=$1
  local demo_directory=$2
  "$TERMTRACE_BIN" index --session "$session" >"$demo_directory/index.json"
  "$TERMTRACE_BIN" inspect --session "$session" --format json >"$demo_directory/final-state.json"
  "$TERMTRACE_BIN" inspect --session "$session" --format text >"$demo_directory/final-screen.txt"
  "$TERMTRACE_BIN" inspect --session "$session" --state 0 --format json >"$demo_directory/initial-state.json"
  "$TERMTRACE_BIN" export-asciicast \
    --session "$session" \
    --out "$demo_directory/session.cast"
  "$TERMTRACE_BIN" capture \
    --session "$session" \
    --width 1920 \
    --height 1080 \
    --fps 24 >"$demo_directory/capture.json"
  ffprobe \
    -v error \
    -select_streams v:0 \
    -show_entries stream=codec_name,pix_fmt,r_frame_rate,avg_frame_rate,width,height,nb_frames \
    -show_entries format=duration \
    -of json \
    "$session/visual/session.mkv" >"$demo_directory/video-probe.json"
  ffmpeg \
    -v error \
    -y \
    -i "$session/visual/session.mkv" \
    -c:v libx264 \
    -preset medium \
    -crf 18 \
    -pix_fmt yuv420p \
    -movflags +faststart \
    "$demo_directory/showcase.mp4"
  chmod 600 "$demo_directory/showcase.mp4"
  jq -s '
    map(select(.event.type? != null))
    | group_by(.event.type)
    | map({type: .[0].event.type, count: length})
  ' "$session/events.jsonl" >"$demo_directory/event-counts.json"
}

print_demo_result() {
  local name=$1
  local directory=$2
  local session=$3
  printf '\n%s demo complete\n' "$name"
  printf '  artifacts:  %s\n' "$directory"
  printf '  video:      %s\n' "$session/visual/session.mkv"
  printf '  showcase:   %s\n' "$directory/showcase.mp4"
  printf '  screenshot: %s\n' "$session/visual/final.png"
  printf '  states:     %s\n' "$session/derived/states.jsonl.zst"
}
