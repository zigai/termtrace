#!/usr/bin/env bash

set -Eeuo pipefail
source "$(dirname "$0")/lib.sh"

umask 077
require_demo_tools jq ffmpeg ffprobe ghostty tmux Xvfb dbus-run-session
build_termtrace

OUTPUT_ROOT=${1:-"$DEMO_REPO_ROOT/demo-artifacts"}
DEMO_DIRECTORY=$OUTPUT_ROOT/03-tmux-recovery
SESSION=$DEMO_DIRECTORY/run
TMUX_SOCKET=termtrace-demo-$$
prepare_demo_directory "$DEMO_DIRECTORY"
trap 'tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true' EXIT

"$TERMTRACE_BIN" record \
  --no-input \
  --out "$SESSION" \
  --geometry fit \
  --viewport 1920x1080 \
  -- tmux \
    -L "$TMUX_SOCKET" \
    -f /dev/null \
    new-session \
    "$DEMO_REPO_ROOT/demos/apps/tmux_showcase.sh" \
  >"$DEMO_DIRECTORY/live-output.ansi" \
  2>"$DEMO_DIRECTORY/recorder.stderr.log"
trap - EXIT

derive_and_capture "$SESSION" "$DEMO_DIRECTORY"

mkdir -m 700 "$DEMO_DIRECTORY/recovered"
cp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/damaged-events.jsonl"
printf '{"truncated_tail":' >>"$DEMO_DIRECTORY/damaged-events.jsonl"
"$TERMTRACE_BIN" recover \
  --event-log "$DEMO_DIRECTORY/damaged-events.jsonl" \
  --out "$DEMO_DIRECTORY/recovered/events.jsonl" \
  >"$DEMO_DIRECTORY/recovery-report.json"
cmp "$SESSION/events.jsonl" "$DEMO_DIRECTORY/recovered/events.jsonl"
"$TERMTRACE_BIN" index --session "$DEMO_DIRECTORY/recovered" \
  --live-snapshot \
  >"$DEMO_DIRECTORY/recovered-index.json"
"$TERMTRACE_BIN" inspect --session "$DEMO_DIRECTORY/recovered" --format text \
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
"$TERMTRACE_BIN" index --session "$DEMO_DIRECTORY/tampered" \
  --live-snapshot \
  >"$DEMO_DIRECTORY/tamper.stdout.log" \
  2>"$DEMO_DIRECTORY/tamper-rejected.log"
TAMPER_STATUS=$?
set -e
if ((TAMPER_STATUS == 0)); then
  printf 'tampered event log was unexpectedly accepted\n' >&2
  exit 1
fi
printf '%s\n' "$TAMPER_STATUS" >"$DEMO_DIRECTORY/tamper-exit-status.txt"

"$TERMTRACE_BIN" schemas --out "$DEMO_DIRECTORY/generated-schemas" \
  >"$DEMO_DIRECTORY/schema-paths.txt"

print_demo_result "tmux + recovery" "$DEMO_DIRECTORY" "$SESSION"
printf '  recovery:   %s\n' "$DEMO_DIRECTORY/recovery-report.json"
printf '  rejection:  %s\n' "$DEMO_DIRECTORY/tamper-rejected.log"
