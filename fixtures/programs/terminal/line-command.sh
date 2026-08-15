#!/bin/sh
set -eu

printf 'LINE_READY\n'
IFS= read -r value
printf 'LINE_VALUE=%s\n' "$value"
