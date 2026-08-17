#!/usr/bin/env sh
set -eu

if [ "$#" -eq 0 ]; then
    exec termtrace doctor
fi

exec termtrace "$@"
