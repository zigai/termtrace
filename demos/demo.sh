#!/usr/bin/env bash

set -Eeuo pipefail
source "$(dirname "$0")/lib.sh"

umask 077
require_demo_tools jq ffmpeg ffprobe ghostty python3 Xvfb dbus-run-session
build_termtrace

OUTPUT_ROOT=${1:-"$DEMO_REPO_ROOT/demo-artifacts"}
DEMO_DIRECTORY=$OUTPUT_ROOT/showcase
SESSION=$DEMO_DIRECTORY/run
STDERR_LOG=$DEMO_DIRECTORY/recorder.stderr.log
prepare_demo_directory "$DEMO_DIRECTORY"

trap 'stop_recorder_on_error "$SESSION"' EXIT
"$TERMTRACE_BIN" record \
    --no-input \
    --out "$SESSION" \
    --geometry fit \
    --viewport 1920x1080 \
    python3 "$DEMO_REPO_ROOT/demos/apps/terminal_showcase.py" \
    >"$DEMO_DIRECTORY/live-output.ansi" \
    2>"$STDERR_LOG" &
RECORDER_PID=$!

wait_for_control "$SESSION" "$STDERR_LOG"

"$TERMTRACE_BIN" status "$SESSION" >"$DEMO_DIRECTORY/status-before.json"

"$TERMTRACE_BIN" wait "$SESSION" --screen SHOWCASE_READY --timeout 15s \
    >"$DEMO_DIRECTORY/wait-ready.json"
"$TERMTRACE_BIN" control mark "$SESSION" --label showcase-ready \
    >"$DEMO_DIRECTORY/mark-ready.json"

"$TERMTRACE_BIN" control resize \
    "$SESSION" \
    --cols 120 \
    --rows 36 \
    --pixel-width 1080 \
    --pixel-height 648 >"$DEMO_DIRECTORY/resize.json"
"$TERMTRACE_BIN" wait "$SESSION" --screen RESIZED:120x36 --timeout 5s \
    >"$DEMO_DIRECTORY/wait-resize.json"

printf 'continue-from-file\n' >"$DEMO_DIRECTORY/agent-input.bin"
"$TERMTRACE_BIN" send \
    "$SESSION" \
    --file "$DEMO_DIRECTORY/agent-input.bin" \
    >"$DEMO_DIRECTORY/send-file.json"
"$TERMTRACE_BIN" wait "$SESSION" --screen AGENT_INPUT:continue-from-file --timeout 5s \
    >"$DEMO_DIRECTORY/wait-input.json"
for signal_name in user1 user2 hangup interrupt; do
    "$TERMTRACE_BIN" control signal "$SESSION" --signal "$signal_name" \
        >"$DEMO_DIRECTORY/signal-$signal_name.json"
done
for signal_name in SIGUSR1 SIGUSR2 SIGHUP SIGINT; do
    "$TERMTRACE_BIN" wait "$SESSION" --screen "SIGNAL:$signal_name" --timeout 5s \
        >"$DEMO_DIRECTORY/wait-$signal_name.json"
done
"$TERMTRACE_BIN" control mark "$SESSION" --label signals-observed \
    >"$DEMO_DIRECTORY/mark-signals.json"

"$TERMTRACE_BIN" stop "$SESSION" >"$DEMO_DIRECTORY/stop.json"
wait_for_recorder "$STDERR_LOG"
trap - EXIT

derive_and_capture "$SESSION" "$DEMO_DIRECTORY"

mkdir -m 700 "$DEMO_DIRECTORY/recovered"
cp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/damaged-events.jsonl"
printf '{"truncated_tail":' >>"$DEMO_DIRECTORY/damaged-events.jsonl"

"$TERMTRACE_BIN" integrity recover \
    --event-log "$DEMO_DIRECTORY/damaged-events.jsonl" \
    --out "$DEMO_DIRECTORY/recovered/events.jsonl" \
    >"$DEMO_DIRECTORY/recovery-report.json"

cmp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/recovered/events.jsonl"

"$TERMTRACE_BIN" analyze index --session "$DEMO_DIRECTORY/recovered" --live-snapshot \
    >"$DEMO_DIRECTORY/recovered-index.json"
"$TERMTRACE_BIN" analyze inspect --session "$DEMO_DIRECTORY/recovered" --format text \
    >"$DEMO_DIRECTORY/recovered-final-screen.txt"
mkdir -m 700 "$DEMO_DIRECTORY/tampered"
while IFS= read -r line; do
    jq -c '
    if .event.type? == "output" then
      .event.data = (if (.event.data | startswith("A")) then "B" else "A" end) + .event.data[1:]
    else
      .
    end
  ' <<<"$line"
done <"$SESSION/events.jsonl" >"$DEMO_DIRECTORY/tampered/events.jsonl"
chmod 600 "$DEMO_DIRECTORY/tampered/events.jsonl"

set +e
"$TERMTRACE_BIN" analyze index --session "$DEMO_DIRECTORY/tampered" --live-snapshot \
    >"$DEMO_DIRECTORY/tamper.stdout.log" \
    2>"$DEMO_DIRECTORY/tamper-rejected.log"
TAMPER_STATUS=$?
set -e

if ((TAMPER_STATUS == 0)); then
    printf 'tampered event log was unexpectedly accepted\n' >&2
    exit 1
fi

printf '%s\n' "$TAMPER_STATUS" >"$DEMO_DIRECTORY/tamper-exit-status.txt"

"$TERMTRACE_BIN" tooling schemas --out "$DEMO_DIRECTORY/generated-schemas" \
    >"$DEMO_DIRECTORY/schema-paths.txt"

print_demo_result "End-to-end" "$DEMO_DIRECTORY" "$SESSION"
printf '  recovery:   %s\n' "$DEMO_DIRECTORY/recovery-report.json"
printf '  rejection:  %s\n' "$DEMO_DIRECTORY/tamper-rejected.log"
