#!/usr/bin/env bash

set -Eeuo pipefail
source "$(dirname "$0")/lib.sh"

umask 077
require_demo_tools jq ffmpeg ffprobe ghostty python3 Xvfb dbus-run-session
build_termtrace

OUTPUT_ROOT=${1:-"$DEMO_REPO_ROOT/demo-artifacts"}
DEMO_DIRECTORY=$OUTPUT_ROOT/01-visual
SESSION=$DEMO_DIRECTORY/run
STDERR_LOG=$DEMO_DIRECTORY/recorder.stderr.log
prepare_demo_directory "$DEMO_DIRECTORY"

trap 'stop_recorder_on_error "$SESSION"' EXIT
"$TERMTRACE_BIN" record \
  --no-input \
  --out "$SESSION" \
  --geometry fit \
  --viewport 1920x1080 \
  -- python3 "$DEMO_REPO_ROOT/demos/apps/terminal_showcase.py" semantic \
  >"$DEMO_DIRECTORY/live-output.ansi" \
  2>"$STDERR_LOG" &
RECORDER_PID=$!

wait_for_control "$SESSION" "$STDERR_LOG"
"$TERMTRACE_BIN" ping "$SESSION" >"$DEMO_DIRECTORY/ping.json"
"$TERMTRACE_BIN" status "$SESSION" >"$DEMO_DIRECTORY/status-before.json"
"$TERMTRACE_BIN" wait "$SESSION" READY_FOR_AGENT --timeout-ms 15000 \
  >"$DEMO_DIRECTORY/wait-ready.json"
"$TERMTRACE_BIN" mark "$SESSION" --label ready-for-agent \
  >"$DEMO_DIRECTORY/mark-ready.json"

"$TERMTRACE_BIN" resize \
  "$SESSION" \
  --cols 120 \
  --rows 36 \
  --pixel-width 1080 \
  --pixel-height 648 >"$DEMO_DIRECTORY/resize.json"
"$TERMTRACE_BIN" wait "$SESSION" RESIZED:120x36 --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-resize.json"

printf 'continue-from-file\n' >"$DEMO_DIRECTORY/agent-input.bin"
"$TERMTRACE_BIN" send \
  "$SESSION" \
  "raw-file:$DEMO_DIRECTORY/agent-input.bin" \
  --intent 'continue the visual showcase with exact file bytes' \
  >"$DEMO_DIRECTORY/send-file.json"
"$TERMTRACE_BIN" wait "$SESSION" AGENT_INPUT:continue-from-file --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-input.json"

"$TERMTRACE_BIN" signal "$SESSION" --signal user1 \
  >"$DEMO_DIRECTORY/signal-user1.json"
"$TERMTRACE_BIN" wait "$SESSION" SEMANTIC_DEMO_COMPLETE --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-complete.json"
"$TERMTRACE_BIN" mark "$SESSION" --label semantic-demo-complete \
  >"$DEMO_DIRECTORY/mark-complete.json"

wait_for_recorder "$STDERR_LOG"
trap - EXIT
derive_and_capture "$SESSION" "$DEMO_DIRECTORY"
print_demo_result "Visual semantics" "$DEMO_DIRECTORY" "$SESSION"
