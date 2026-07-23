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
  'printf "\033[2J\033[H\033[38;2;255;0;0mLong preroll\033[0m\n"; sleep 12; printf "\033[38;2;0;255;0mSelected range\033[0m\n"; sleep 0.5; printf "excluded tail\n"'
rm -f "$profile"
target/debug/termtrace analyze index --session "$output"
target/debug/termtrace artifact capture --session "$output" --fps 24
target/debug/termtrace artifact screenshot "$output" --renderer native \
  --at 12.2s --viewport 640x360 --strict --out "$output/native-single"

readonly range_from=11000000000
readonly range_to=12350000000
readonly range_suffix="range-${range_from}-${range_to}"
readonly range_derived="$output/derived-$range_suffix"
readonly range_visual="$output/visual-$range_suffix"
target/debug/termtrace analyze index --session "$output" \
  --from "${range_from}ns" --to "${range_to}ns"
readonly range_capture_started="$(date +%s%N)"
target/debug/termtrace artifact capture --session "$output" --fps 24 \
  --from "${range_from}ns" --to "${range_to}ns"
readonly range_capture_elapsed_ns="$(( $(date +%s%N) - range_capture_started ))"
test "$range_capture_elapsed_ns" -lt "$range_from"

readonly manifest="$output/visual/manifest.json"
jq -e '
  .fps == 24 and
  .source_range == null and
  .width == 640 and
  .height == 360 and
  .video_probe.streams[0].codec_name == "h264" and
  .video_probe.streams[0].avg_frame_rate == "24/1" and
  (.video_probe.streams[0].nb_read_packets | tonumber) > 0 and
  .screenshot_probe.streams[0].codec_name == "png" and
  .screenshot_probe.streams[0].width == 640 and
  .screenshot_probe.streams[0].height == 360
' "$manifest" >/dev/null

readonly range_manifest="$range_visual/manifest.json"
jq -e --argjson from "$range_from" --argjson to "$range_to" '
  .source_range.start_monotonic_ns == $from and
  .source_range.end_monotonic_ns == $to and
  .fps == 24 and
  .video_probe.streams[0].avg_frame_rate == "24/1" and
  (.video_probe.streams[0].nb_read_packets | tonumber) > 0 and
  .screenshot_probe.streams[0].codec_name == "png"
' "$range_manifest" >/dev/null
jq -s -e --argjson from "$range_from" --argjson to "$range_to" '
  length > 0 and all(.[]; .monotonic_ns >= $from and .monotonic_ns < $to)
' "$range_visual/video-map.jsonl" >/dev/null
jq -e --argjson from "$range_from" --argjson to "$range_to" '
  .source_range.start_monotonic_ns == $from and
  .source_range.end_monotonic_ns == $to
' "$range_derived/manifest.json" >/dev/null
grep -q 'Long preroll' "$range_derived/final.txt"
grep -q 'Selected range' "$range_derived/final.txt"
! grep -q 'excluded tail' "$range_derived/final.txt"

readonly full_duration="$(jq -r '.video_probe.format.duration | tonumber' "$manifest")"
readonly range_duration="$(jq -r '.video_probe.format.duration | tonumber' "$range_manifest")"
awk -v full="$full_duration" -v range="$range_duration" 'BEGIN { exit !(range + 5 < full) }'

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
test -s "$output/native-single/final.png"
test ! -e "$output/native-single/session.mkv"
jq -e '
  .capture_kind == "screenshot" and
  .presentation_evidence == "post_endpoint_stable_capture" and
  .guarantee_level == "native_capture_environment" and
  .selected_state_hash != null and
  .endpoint_event_seq != null and
  (.frame_samples | length) >= 3 and
  .frame_samples[-1].same_as_previous == true and
  .video_probe == null
' "$output/native-single/manifest.json" >/dev/null
test -s "$range_visual/session.mkv"
test -s "$range_visual/final.png"
test -s "$range_visual/video-map.jsonl"
! cmp -s "$output/visual/final.png" "$range_visual/final.png"
awk -v elapsed="$range_capture_elapsed_ns" -v preroll="$range_from" \
  'BEGIN { printf "ranged capture wall time: %.3fs for %.3fs pre-roll\n", elapsed / 1000000000, preroll / 1000000000 }'
printf 'native capture smoke passed: %s\n' "$output"
