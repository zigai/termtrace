#!/bin/sh
# Run against a tmux binary built with patches/tmux/pane-launch-command.patch.
set -eu
tmux_binary=${1:?usage: check-tmux-launch-hook.sh /absolute/path/to/tmux /absolute/path/to/termtrace}
recorder=${2:?a built Termtrace executable is required}
root=$(mktemp -d)
socket=termtrace-launch-check-$$
cleanup() {
    "$tmux_binary" -L "$socket" kill-server >/dev/null 2>&1 || :
    rm -rf "$root"
}
trap cleanup EXIT HUP INT TERM
observer_pid() {
    "$recorder" tmux status --json --tmux "$tmux_binary" -L "$socket" |
        sed -n "s/.*\"$1\": *\([0-9][0-9]*\).*/\1/p"
}
wait_completed() {
    remaining=200
    while [ "$remaining" -gt 0 ]; do
        count=0
        for completion in "$root"/state/sessions/*.termtrace/completion.json; do
            if [ -f "$completion" ]; then count=$((count + 1)); fi
        done
        if [ "$count" -eq "$1" ]; then return; fi
        remaining=$((remaining - 1))
        sleep 0.05
    done
    printf 'Timed out waiting for %s completed recordings\n' "$1" >&2
    exit 1
}
"$recorder" -F tmux config --tmux "$tmux_binary" --launch-hook \
    --ghostty-profile off --output-root "$root/state" >"$root/tmux.conf"
printf '%s\n' 'set-option -g default-shell /bin/sh' \
    'set-window-option -g remain-on-exit on' >>"$root/tmux.conf"
"$tmux_binary" -L "$socket" -f "$root/tmux.conf" new-session -d -s recorded \
    /bin/sh -c 'printf "IMMEDIATE_OUTPUT\n"'
wait_completed 1
"$tmux_binary" -L "$socket" respawn-pane -k -t recorded \
    /bin/sh -c 'printf "RESPAWN_OUTPUT\n"'
wait_completed 2
"$tmux_binary" -L "$socket" new-window -d -n shell 'printf "SHELL_OUTPUT\n"'
wait_completed 3
"$tmux_binary" -L "$socket" new-window -d -n login
"$tmux_binary" -L "$socket" send-keys -t recorded:login \
    'printf "LOGIN_ARGV=%s\n" "$0"; exit' Enter
wait_completed 4
"$tmux_binary" -L "$socket" new-window -d -n args /usr/bin/printf '<%s>\n' \
    '' 'literal $(false); "quote" *'
wait_completed 5
"$tmux_binary" -L "$socket" split-window -d -t recorded:0 \
    /bin/sh -c 'printf "SPLIT_OUTPUT\n"'
wait_completed 6
"$tmux_binary" -L "$socket" respawn-window -k -t recorded:shell \
    /bin/sh -c 'printf "WINDOW_RESPAWN_OUTPUT\n"'
wait_completed 7
"$tmux_binary" -L "$socket" new-session -d -s second \
    /bin/sh -c 'printf "SECOND_SESSION_OUTPUT\n"'
wait_completed 8
remaining=200
observer=
while [ "$remaining" -gt 0 ]; do
    observer=$(observer_pid supervisor)
    if [ -n "$observer" ] && kill -0 "$observer" 2>/dev/null; then break; fi
    remaining=$((remaining - 1))
    sleep 0.05
done
test "$remaining" -gt 0
watchdog=$(observer_pid watchdog)
kill -0 "$watchdog"
kill -KILL "$observer"
remaining=200
while [ "$remaining" -gt 0 ]; do
    replacement=$(observer_pid supervisor)
    if [ -n "$replacement" ] && [ "$replacement" != "$observer" ] && kill -0 "$replacement" 2>/dev/null; then break; fi
    remaining=$((remaining - 1))
    sleep 0.05
done
test "$remaining" -gt 0
test "$(observer_pid watchdog)" = "$watchdog"
for session in "$root"/state/sessions/*.termtrace; do
    "$recorder" show "$session" >>"$root/output"
done
for expected in IMMEDIATE_OUTPUT RESPAWN_OUTPUT SHELL_OUTPUT LOGIN_ARGV=-sh \
    '<>' '<literal $(false); "quote" *>' SPLIT_OUTPUT WINDOW_RESPAWN_OUTPUT SECOND_SESSION_OUTPUT; do
    grep -Fx "$expected" "$root/output" >/dev/null
done
"$recorder" tmux disable --tmux "$tmux_binary" -L "$socket" >"$root/disable.json"
test -z "$("$tmux_binary" -L "$socket" show-options -gv pane-launch-command)"
printf '%s\n' 'Launch hook: immediate output, pane/window respawn, split, sessions, literal argv, shell, login, observer crash recovery and restoration passed.'
