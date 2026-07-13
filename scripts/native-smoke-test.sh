#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly output="${1:-$(mktemp -d)/native-smoke.termtrace}"

cd "$root"
cargo build
target/debug/termtrace record --no-input --out "$output" -- /bin/sh -c \
  'printf "\033[2J\033[H\033[38;2;255;0;0mNative Ghostty\033[0m\n"; sleep 0.25; printf "\033[38;2;0;255;0m24 FPS capture\033[0m\n"; sleep 0.25; printf "done\n"'
target/debug/termtrace index --session "$output"
target/debug/termtrace capture --session "$output" --width 640 --height 360 --fps 24

readonly manifest="$output/visual/manifest.json"
jq -e '
  .fps == 24 and
  .width == 640 and
  .height == 360 and
  .video_probe.streams[0].codec_name == "h264" and
  .video_probe.streams[0].avg_frame_rate == "24/1" and
  (.video_probe.streams[0].nb_read_frames | tonumber) > 0 and
  .screenshot_probe.streams[0].codec_name == "png" and
  .screenshot_probe.streams[0].width == 640 and
  .screenshot_probe.streams[0].height == 360
' "$manifest" >/dev/null

test -s "$output/visual/session.mkv"
test -s "$output/visual/final.png"
test -s "$output/visual/video-map.jsonl"
printf 'native capture smoke passed: %s\n' "$output"
