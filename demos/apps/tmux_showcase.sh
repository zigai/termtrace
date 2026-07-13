#!/usr/bin/env bash

set -Eeuo pipefail

printf '\033[2J\033[H\033]0;Termtrace tmux demo\033\\'
printf '\033[1;38;2;102;204;255mTERMTRACE / TMUX TARGET\033[0m\n\n'
printf 'Termtrace owns the outer PTY; tmux remains an optional target.\n'
printf 'Canonical evidence still comes from exact outer-PTY bytes.\n\n'
for percent in 10 25 50 75 100; do
  printf '\r\033[38;5;82mPane workload: [%3s%%] %-20s\033[0m' "$percent" "$(printf '%*s' $((percent / 5)) '' | tr ' ' '#')"
  sleep 0.25
done
printf '\n\n\033[1;38;5;214mTMUX_DEMO_COMPLETE\033[0m\n'
sleep 1
