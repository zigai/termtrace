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
  --cols 100 \
  --rows 30 \
  --cell-width 9 \
  --cell-height 18 \
  -- python3 "$DEMO_REPO_ROOT/demos/apps/terminal_showcase.py" semantic \
  >"$DEMO_DIRECTORY/live-output.ansi" \
  2>"$STDERR_LOG" &
RECORDER_PID=$!

wait_for_control "$SESSION" "$STDERR_LOG"
"$TERMTRACE_BIN" ping --session "$SESSION" >"$DEMO_DIRECTORY/ping.json"
"$TERMTRACE_BIN" status --session "$SESSION" >"$DEMO_DIRECTORY/status-before.json"
"$TERMTRACE_BIN" wait --session "$SESSION" --text READY_FOR_AGENT --timeout-ms 15000 \
  >"$DEMO_DIRECTORY/wait-ready.json"
"$TERMTRACE_BIN" mark --session "$SESSION" --label ready-for-agent \
  >"$DEMO_DIRECTORY/mark-ready.json"

"$TERMTRACE_BIN" resize \
  --session "$SESSION" \
  --cols 120 \
  --rows 36 \
  --pixel-width 1080 \
  --pixel-height 648 >"$DEMO_DIRECTORY/resize.json"
"$TERMTRACE_BIN" wait --session "$SESSION" --text RESIZED:120x36 --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-resize.json"

printf 'continue-from-file\n' >"$DEMO_DIRECTORY/agent-input.bin"
"$TERMTRACE_BIN" send \
  --session "$SESSION" \
  --file "$DEMO_DIRECTORY/agent-input.bin" \
  --intent 'continue the visual showcase with exact file bytes' \
  >"$DEMO_DIRECTORY/send-file.json"
"$TERMTRACE_BIN" wait --session "$SESSION" --text AGENT_INPUT:continue-from-file --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-input.json"

"$TERMTRACE_BIN" signal --session "$SESSION" --signal user1 \
  >"$DEMO_DIRECTORY/signal-user1.json"
"$TERMTRACE_BIN" wait --session "$SESSION" --text SEMANTIC_DEMO_COMPLETE --timeout-ms 5000 \
  >"$DEMO_DIRECTORY/wait-complete.json"
"$TERMTRACE_BIN" mark --session "$SESSION" --label semantic-demo-complete \
  >"$DEMO_DIRECTORY/mark-complete.json"

wait_for_recorder "$STDERR_LOG"
trap - EXIT
derive_and_capture "$SESSION" "$DEMO_DIRECTORY"
print_demo_result "Visual semantics" "$DEMO_DIRECTORY" "$SESSION"
