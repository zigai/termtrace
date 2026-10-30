#!/usr/bin/env bash
set -euo pipefail

readonly zig_version=0.17.0
readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ghostty="$root/vendor/ghostty"
readonly tools="$root/.tools"
readonly revision="$(<"$root/crates/terminal/ghostty-revision")"

if [[ "$(uname -s)" != Linux ]]; then
    echo "libghostty-vt bootstrap currently supports Linux only" >&2
    exit 2
fi

case "$(uname -m)" in
    x86_64)
        readonly zig_arch=x86_64
        readonly zig_sha256=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
        ;;
    aarch64)
        readonly zig_arch=aarch64
        readonly zig_sha256=9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8
        ;;
    *)
        echo "unsupported architecture: $(uname -m)" >&2
        exit 2
        ;;
esac

if [[ ! -f "$ghostty/build.zig" ]]; then
    echo "Ghostty submodule is missing; run: git submodule update --init --recursive" >&2
    exit 2
fi

readonly archive_name="zig-${zig_arch}-linux-${zig_version}.tar.xz"
readonly archive="$tools/downloads/$archive_name"
readonly zig_root="$tools/zig-${zig_arch}-linux-${zig_version}"
readonly zig="$zig_root/zig"

mkdir -p "$tools/downloads"
if [[ ! -x "$zig" ]]; then
    curl --fail --location --retry 3 \
        --output "$archive" \
        "https://ziglang.org/download/${zig_version}/${archive_name}"
    printf '%s  %s\n' "$zig_sha256" "$archive" | sha256sum --check --status
    tar --extract --xz --directory "$tools" --file "$archive"
fi

if [[ "$($zig version)" != "$zig_version" ]]; then
    echo "unexpected Zig version from $zig" >&2
    exit 2
fi

cd "$ghostty"

optimize_mode="ReleaseFast"
if [[ "${TERMTRACE_SANITIZE:-}" == "address" || "${TERMTRACE_ZIG_SANITIZE:-}" == "address" ]]; then
    optimize_mode="ReleaseSafe"
fi
if [[ -n "${TERMTRACE_ZIG_OPTIMIZE:-}" ]]; then
    optimize_mode="${TERMTRACE_ZIG_OPTIMIZE}"
fi

target_args=()
if [[ -n "${TERMTRACE_ZIG_TARGET:-}" ]]; then
    target_args+=("-Dtarget=${TERMTRACE_ZIG_TARGET}")
fi

extra_args=()
if [[ -n "${TERMTRACE_ZIG_EXTRA_ARGS:-}" ]]; then
    read -r -a extra_args <<<"${TERMTRACE_ZIG_EXTRA_ARGS}"
fi

readonly output="${TERMTRACE_GHOSTTY_PREFIX:-$ghostty/zig-out}"

"$zig" build -Demit-lib-vt=true "-Doptimize=${optimize_mode}" --prefix "$output" "${target_args[@]}" "${extra_args[@]}"

test -f "$output/lib/libghostty-vt.a"
test -f "$output/include/ghostty/vt.h"

# crates/terminal/build.rs links only a prefix whose stamp matches the pinned revision.
git -C "$ghostty" rev-parse HEAD >"$output/ghostty-revision"

printf 'libghostty-vt ready at %s\n' "$output"

if [[ "$optimize_mode" != ReleaseFast || ${#target_args[@]} -gt 0 || ${#extra_args[@]} -gt 0 ]]; then
    exit 0
fi

if [[ "$(git -C "$ghostty" rev-parse HEAD)" != "$revision" ]]; then
    echo "Ghostty is not at the pinned revision $revision; skipping the shared libghostty-vt cache" >&2
    exit 0
fi

readonly cache="${XDG_CACHE_HOME:-$HOME/.cache}/termtrace/libghostty-vt/$revision"
mkdir -p "$(dirname "$cache")"
staging="$(mktemp -d "$cache.XXXXXX")"
cp -R "$output/lib" "$output/include" "$output/ghostty-revision" "$staging/"
rm -rf "$cache"
mv "$staging" "$cache"

printf 'libghostty-vt shared at %s\n' "$cache"
