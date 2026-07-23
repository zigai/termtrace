#!/usr/bin/env bash
set -euo pipefail

case "$(uname -m)" in
    x86_64 | amd64) target=x86_64-unknown-linux-gnu ;;
    aarch64 | arm64) target=aarch64-unknown-linux-gnu ;;
    *)
        echo "unsupported test architecture: $(uname -m)" >&2
        exit 2
        ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
assets="$work/assets"
root="$assets/termtrace-$target"
install_dir="$work/bin"
mkdir -p "$root/clients/python" "$root/clients/typescript/lib" "$install_dir"
printf '[project]\nname="termtrace"\nversion="0.0.0"\n' >"$root/clients/python/pyproject.toml"
printf 'export {}\n' >"$root/clients/typescript/lib/index.js"

build_archive() {
    archive="$assets/termtrace-$target.tar.gz"
    tar -C "$assets" -czf "$archive" "termtrace-$target"
    (cd "$assets" && sha256sum "termtrace-$target.tar.gz" >"termtrace-$target.tar.gz.sha256")
}

printf '#!/bin/sh\nexit 0\n' >"$root/termtrace"
chmod 0555 "$root/termtrace"
build_archive
TERMTRACE_DOWNLOAD_BASE="file://$assets" ./install.sh --install-dir "$install_dir"
cmp "$root/termtrace" "$install_dir/termtrace"
test -f "$work/share/termtrace/clients/python/pyproject.toml"
test -f "$work/share/termtrace/clients/typescript/lib/index.js"

rm -f "$root/termtrace"
printf '#!/bin/sh\nexit 1\n' >"$root/termtrace"
chmod 0555 "$root/termtrace"
build_archive
if TERMTRACE_DOWNLOAD_BASE="file://$assets" ./install.sh --install-dir "$install_dir"; then
    echo "installer accepted a candidate that failed its capability check" >&2
    exit 1
fi
printf '#!/bin/sh\nexit 0\n' | cmp - "$install_dir/termtrace"
test -z "$(find "$install_dir" -name '.termtrace.*' -print -quit)"
