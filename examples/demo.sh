#!/usr/bin/env bash

set -Eeuo pipefail
source "$(dirname "$0")/lib.sh"

umask 077
require_demo_tools jq ffprobe ghostty python3 Xvfb dbus-run-session
if [[ -n $DEMO_BUILD_RELEASE ]]; then
    cargo build --manifest-path "$DEMO_REPO_ROOT/Cargo.toml" --locked --release
fi

OUTPUT_ROOT=${1:-"$DEMO_REPO_ROOT/demo-artifacts"}
DEMO_DIRECTORY=$OUTPUT_ROOT/showcase
SESSION=$DEMO_DIRECTORY/run
PROFILE='{"version":1,"native":{"font_family":"JetBrains Mono","font_size_points":"12","dpi":96}}'
prepare_demo_directory "$DEMO_DIRECTORY"
# Keep the background session's alias out of the user's registry.
export TERMTRACE_STATE_DIR=$DEMO_DIRECTORY/state

"$TERMTRACE_BIN" doctor --require native-video --json >"$DEMO_DIRECTORY/doctor-native-video.json"
"$TERMTRACE_BIN" doctor --require native-screenshot --json >"$DEMO_DIRECTORY/doctor-native-screenshot.json"

trap 'stop_session_on_error "$SESSION"' EXIT
"$TERMTRACE_BIN" record \
    --original-native \
    --original-grid 80x24 \
    --original-canvas 1280x720 \
    --original-fps 24 \
    --visual-profile "$PROFILE" \
    --out "$SESSION" \
    --detach showcase \
    --until-screen SHOWCASE_READY \
    --timeout 30s \
    --json \
    python3 "$DEMO_REPO_ROOT/examples/apps/terminal_showcase.py" \
    </dev/null >"$DEMO_DIRECTORY/record.json"

"$TERMTRACE_BIN" send "$SESSION" --mark showcase-ready --json >"$DEMO_DIRECTORY/mark-ready.json"
"$TERMTRACE_BIN" send "$SESSION" --text $'continue-from-file\n' \
    --until-screen WAITING_FOR_SIGNALS --json >"$DEMO_DIRECTORY/send-text.json"
for signal_name in user1:SIGUSR1 user2:SIGUSR2 hangup:SIGHUP interrupt:SIGINT; do
    "$TERMTRACE_BIN" send "$SESSION" --signal "${signal_name%%:*}" \
        --until-screen "SIGNAL:${signal_name#*:}" --json >"$DEMO_DIRECTORY/signal-${signal_name%%:*}.json"
done
"$TERMTRACE_BIN" send "$SESSION" --mark signals-observed --json >"$DEMO_DIRECTORY/mark-signals.json"

"$TERMTRACE_BIN" stop "$SESSION" --wait --json >"$DEMO_DIRECTORY/stop.json"
wait_for_published_capture "$SESSION"
trap - EXIT

export_edited_videos "$SESSION" "$DEMO_DIRECTORY"
"$TERMTRACE_BIN" artifact asciicast "$SESSION" --out "$DEMO_DIRECTORY/session.cast"
"$TERMTRACE_BIN" analyze inspect "$SESSION" --format text >"$DEMO_DIRECTORY/final-screen.txt"

mkdir -m 700 "$DEMO_DIRECTORY/recovered"
cp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/damaged-events.jsonl"
printf '{"truncated_tail":' >>"$DEMO_DIRECTORY/damaged-events.jsonl"

"$TERMTRACE_BIN" integrity recover \
    --event-log "$DEMO_DIRECTORY/damaged-events.jsonl" \
    --out "$DEMO_DIRECTORY/recovered/events.jsonl" \
    >"$DEMO_DIRECTORY/recovery-report.json"

cmp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/recovered/events.jsonl"

mkdir -m 700 "$DEMO_DIRECTORY/tampered"
jq -c '
  if .event.type? == "output" then
    .event.data = (if (.event.data | startswith("A")) then "B" else "A" end) + .event.data[1:]
  else
    .
  end
' "$SESSION/events.jsonl" >"$DEMO_DIRECTORY/tampered/events.jsonl"
chmod 600 "$DEMO_DIRECTORY/tampered/events.jsonl"

set +e
"$TERMTRACE_BIN" analyze index "$DEMO_DIRECTORY/tampered" --live-snapshot \
    >"$DEMO_DIRECTORY/tamper.stdout.log" \
    2>"$DEMO_DIRECTORY/tamper-rejected.log"
TAMPER_STATUS=$?
set -e

if ((TAMPER_STATUS == 0)); then
    printf 'tampered event log was unexpectedly accepted\n' >&2
    exit 1
fi

printf '%s\n' "$TAMPER_STATUS" >"$DEMO_DIRECTORY/tamper-exit-status.txt"

"$TERMTRACE_BIN" schema --out "$DEMO_DIRECTORY/generated-schemas" >"$DEMO_DIRECTORY/schema-paths.txt"

print_demo_result "End-to-end" "$DEMO_DIRECTORY" "$SESSION"
printf '  recovery:   %s\n' "$DEMO_DIRECTORY/recovery-report.json"
printf '  rejection:  %s\n' "$DEMO_DIRECTORY/tamper-rejected.log"
