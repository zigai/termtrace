#!/usr/bin/env bash
# Run the Rust, Python and TypeScript demo drivers one after another and check their media.
set -euo pipefail
umask 077

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly output="${1:-$root/target/sdk-demos/$(date +%Y%m%d-%H%M%S)}"
export TERMTRACE_BIN="${TERMTRACE_BIN:-$root/target/debug/termtrace}"
cd "$root"

if [[ -e $output ]]; then
    printf 'demo output already exists: %s\n' "$output" >&2
    exit 1
fi
mkdir -p "$output"

"$TERMTRACE_BIN" doctor --require native-video,native-screenshot,presentation --json >"$output/doctor.json"

cargo build --locked --quiet --manifest-path tests/consumers/rust/Cargo.toml --features local --bin demo
tests/consumers/node_native/node_modules/.bin/tsc -p tests/consumers/node_native/tsconfig.examples.json

tests/consumers/rust/target/debug/demo "$output/rust" >"$output/rust.json"
uv run --project tests/consumers/python_native python examples/sdk/python/demo.py "$output/python" >"$output/python.json"
node tests/consumers/node_native/.examples/demo.js "$output/typescript" >"$output/typescript.json"

declare -A codecs=([mp4]=h264 [gif]=gif [webm]=vp9)
for language in rust python typescript; do
    jq -e '.contact_sheet' "$output/$language.json" | xargs test -s
    for extension in mp4 gif webm; do
        video="$output/$language/renders/demo.$extension"
        plan="$video.termtrace/normalized-plan.json"
        expected="${codecs[$extension]},$(jq -r '"\(.output_width),\(.output_height),\(.output_frame_count)"' "$plan")"
        actual="$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=codec_name,width,height,nb_read_frames -of csv=p=0 "$video")"
        if [[ $actual != "$expected" ]]; then
            printf '%s: expected %s, decoded %s\n' "$video" "$expected" "$actual" >&2
            exit 1
        fi
        printf '%s %s %s\n' "$language" "$extension" "$actual" >>"$output/probe.txt"
    done
done
cat "$output/probe.txt"
printf 'demo outputs: %s\n' "$output"
