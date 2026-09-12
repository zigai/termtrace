#!/usr/bin/env bash
set -euo pipefail

readonly zig_version=0.15.2
readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ghostty="$root/vendor/ghostty"
readonly tools="$root/.tools"

if [[ "$(uname -s)" != Linux ]]; then
    echo "libghostty-vt bootstrap currently supports Linux only" >&2
    exit 2
fi

case "$(uname -m)" in
    x86_64)
        readonly zig_arch=x86_64
        readonly zig_sha256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
        ;;
    aarch64)
        readonly zig_arch=aarch64
        readonly zig_sha256=958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f
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

"$zig" build -Demit-lib-vt=true "-Doptimize=${optimize_mode}" "${target_args[@]}" "${extra_args[@]}"

test -f zig-out/lib/libghostty-vt.a
test -f zig-out/include/ghostty/vt.h

printf 'libghostty-vt ready at %s\n' "$ghostty/zig-out"
