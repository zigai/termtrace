#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
binary=${TERMTRACE_BINARY:-$root/target/release/termtrace}
contract=${CONTRACT:-$root/benchmarks/agent-workflows-v1.json}
output=${OUTPUT:-$root/benchmarks/results/agent-workflows.jsonl}

command -v jq >/dev/null 2>&1 || {
    echo "jq is required" >&2
    exit 2
}

test -x "$binary" || {
    echo "Termtrace binary is not executable: $binary" >&2
    exit 2
}

jq -e '.format == "termtrace-agent-workflow-benchmark" and .version == 1' "$contract" >/dev/null

work=$(mktemp -d)
recorder=

cleanup() {
    if [[ -n "$recorder" ]]; then kill "$recorder" 2>/dev/null || true; fi
    rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

session=$work/agent-workflow.termtrace

"$binary" record --no-input --cols 80 --rows 24 --out "$session" -- /bin/sh -c \
    'trap "exit 0" TERM; printf Ready; IFS= read -r line; printf "Done\n"; i=0; while [ "$i" -lt 260 ]; do printf "\r%03d" "$i"; i=$((i + 1)); done; while :; do sleep .1; done' \
    >"$work/record.stdout" 2>"$work/record.stderr" &
recorder=$!

ready=false
for _ in $(seq 1 500); do
    if "$binary" status "$session" >/dev/null 2>&1; then
        ready=true
        break
    fi
    sleep 0.01
done

[[ "$ready" == true ]] || {
    echo "fixture did not become ready" >&2
    exit 1
}
measure() {
    local id=$1
    shift
    local stdout=$work/$id.stdout
    local started ended maximum maximum_commands bytes command_count
    started=$(date +%s%N)
    "$@" >"$stdout"
    ended=$(date +%s%N)
    bytes=$(wc -c <"$stdout")
    maximum=$(jq -r --arg id "$id" '.workflows[] | select(.id == $id) | .maximum_stdout_bytes' "$contract")
    maximum_commands=$(jq -r --arg id "$id" '.workflows[] | select(.id == $id) | .maximum_commands' "$contract")
    command_count=1
    jq -cn \
        --arg id "$id" \
        --argjson stdout_bytes "$bytes" \
        --argjson elapsed_ns "$((ended - started))" \
        --argjson maximum_stdout_bytes "$maximum" \
        --argjson command_count "$command_count" \
        --argjson maximum_commands "$maximum_commands" \
        '{workflow:$id,stdout_bytes:$stdout_bytes,elapsed_ns:$elapsed_ns,maximum_stdout_bytes:$maximum_stdout_bytes,command_count:$command_count,maximum_commands:$maximum_commands,passed:($stdout_bytes <= $maximum_stdout_bytes and $command_count <= $maximum_commands)}' |
        tee -a "$output"
    ((bytes <= maximum && command_count <= maximum_commands))
}

mkdir -p "$(dirname "$output")"
: >"$output"
measure stable-observe "$binary" wait "$session" --idle 10ms
jq -e '.kind == "stable_snapshot" and (has("semantic_state_zstd") | not) and .session_state == "unknown" and .state != null' "$work/stable-observe.stdout" >/dev/null

measure atomic-input-sequence "$binary" --fields boundary act "$session" text:go enter
jq -e '.type == "action" and .input.atoms == 2 and .boundaries != null' "$work/atomic-input-sequence.stdout" >/dev/null

"$binary" wait "$session" --screen Done --timeout 5s >/dev/null
"$binary" wait "$session" --screen 259 --timeout 5s >/dev/null
measure live-status "$binary" status "$session"
jq -e '.kind == "status" and .lifecycle == "live" and .session_state == "live" and (has("boundaries") | not)' "$work/live-status.stdout" >/dev/null

"$binary" stop "$session" --wait >/dev/null
wait "$recorder"
recorder=

measure bounded-timeline "$binary" analyze timeline "$session" --jsonl
jq -e 'select(.kind == "output_page") | .page.truncated == true and .page.limit == 100 and .page.next_offset == 100' "$work/bounded-timeline.stdout" >/dev/null

client_worktree=$work/client-worktree
client_script=$work/client-workflow.py

mkdir -p "$client_worktree"
printf '%s\n' 'from termtrace import ProtocolMismatchError' 'print(ProtocolMismatchError.__name__)' >"$client_script"

pushd "$client_worktree" >/dev/null
measure managed-client-python "$binary" tooling client python "$client_script"
popd >/dev/null

test -z "$(find "$client_worktree" -mindepth 1 -print -quit)"
