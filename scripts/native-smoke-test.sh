#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly output="${1:-$(mktemp -d)/native-smoke.termtrace}"
readonly profile="$(mktemp)"

trap 'rm -f "$profile"' EXIT
printf 'font-size = 11\nwindow-padding-x = 2\nbackground = #220000\n' >"$profile"

cd "$root"
cargo build
target/debug/termtrace record --no-input --geometry fit --viewport 640x360 \
  --ghostty-config "$profile" --font-size 13 --padding-x 6 --padding-y 4 \
  --foreground '#e6e6e6' --background '#101010' --out "$output" -- /bin/sh -c \
  'printf "\033[2J\033[H\033[38;2;255;0;0mNative Ghostty\033[0m\n"; sleep 0.25; printf "\033[38;2;0;255;0m24 FPS capture\033[0m\n"; sleep 0.25; printf "done\n"'
rm -f "$profile"
target/debug/termtrace index --session "$output"
target/debug/termtrace capture --session "$output" --fps 24

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

jq -e '
  .ghostty_visual.source_config != null and
  .ghostty_visual.overrides.font_size == "13" and
  .ghostty_visual.overrides.padding_x == 6 and
  .ghostty_visual.overrides.padding_y == 4 and
  .ghostty_visual.overrides.foreground == "#e6e6e6" and
  .ghostty_visual.overrides.background == "#101010" and
  .ghostty_visual.effective_config == "ghostty-config.txt"
' "$output/session.json" >/dev/null
test "$(jq -r '.ghostty_visual.effective_config_hash' "$output/session.json")" = \
  "$(jq -r '.ghostty_config_hash' "$manifest")"
test ! -e "$profile"
test -s "$output/ghostty-config.txt"
! grep -Eq '^config-(file|default-files)[[:space:]]*=' "$output/ghostty-config.txt"

test -s "$output/visual/session.mkv"
test -s "$output/visual/final.png"
test -s "$output/visual/video-map.jsonl"
printf 'native capture smoke passed: %s\n' "$output"
