#!/usr/bin/env bash

set -Eeuo pipefail
source "$(dirname "$0")/lib.sh"

umask 077
require_demo_tools jq ffmpeg ffprobe ghostty python3 Xvfb dbus-run-session
build_termtrace

OUTPUT_ROOT=${1:-"$DEMO_REPO_ROOT/demo-artifacts"}
DEMO_DIRECTORY=$OUTPUT_ROOT/02-control
SESSION=$DEMO_DIRECTORY/run
STDERR_LOG=$DEMO_DIRECTORY/recorder.stderr.log
prepare_demo_directory "$DEMO_DIRECTORY"

trap 'stop_recorder_on_error "$SESSION"' EXIT
"$TERMTRACE_BIN" record \
  --no-input \
  --out "$SESSION" \
  --geometry fit \
  --viewport 1920x1080 \
  -- python3 "$DEMO_REPO_ROOT/demos/apps/terminal_showcase.py" control \
  >"$DEMO_DIRECTORY/live-output.ansi" \
  2>"$STDERR_LOG" &
RECORDER_PID=$!

wait_for_control "$SESSION" "$STDERR_LOG"
"$TERMTRACE_BIN" ping "$SESSION" >"$DEMO_DIRECTORY/ping.json"
"$TERMTRACE_BIN" status "$SESSION" >"$DEMO_DIRECTORY/status.json"
"$TERMTRACE_BIN" wait "$SESSION" CONTROL_READY --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-ready.json"
"$TERMTRACE_BIN" mark "$SESSION" --label controls-begin \
  >"$DEMO_DIRECTORY/mark-begin.json"

"$TERMTRACE_BIN" send \
  "$SESSION" \
  "text:"$'hello-from-agent\n' \
  --intent 'demonstrate exact UTF-8 text input' >"$DEMO_DIRECTORY/send-text.json"
"$TERMTRACE_BIN" wait "$SESSION" INPUT:hello-from-agent --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-input.json"

for signal_name in user2 hangup interrupt; do
  "$TERMTRACE_BIN" signal "$SESSION" --signal "$signal_name" \
    >"$DEMO_DIRECTORY/signal-$signal_name.json"
done
"$TERMTRACE_BIN" wait "$SESSION" SIGNAL:SIGUSR2 --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-user2.json"
"$TERMTRACE_BIN" wait "$SESSION" SIGNAL:SIGHUP --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-hangup.json"
"$TERMTRACE_BIN" wait "$SESSION" SIGNAL:SIGINT --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-interrupt.json"
"$TERMTRACE_BIN" mark "$SESSION" --label signals-observed \
  >"$DEMO_DIRECTORY/mark-signals.json"

"$TERMTRACE_BIN" stop "$SESSION" >"$DEMO_DIRECTORY/stop.json"
wait_for_recorder "$STDERR_LOG"
trap - EXIT
derive_and_capture "$SESSION" "$DEMO_DIRECTORY"
print_demo_result "Control lifecycle" "$DEMO_DIRECTORY" "$SESSION"
