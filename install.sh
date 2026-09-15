#!/usr/bin/env bash
set -euo pipefail

repository=${TERMTRACE_REPOSITORY:-zigai/termtrace}
version=${TERMTRACE_VERSION:-latest}
download_base=${TERMTRACE_DOWNLOAD_BASE:-}

if [[ -n "${TERMTRACE_INSTALL_DIR:-}" ]]; then
    install_dir=$TERMTRACE_INSTALL_DIR
elif [[ -n "${HOME:-}" ]]; then
    install_dir="${HOME}/.local/bin"
else
    echo "HOME or TERMTRACE_INSTALL_DIR is required" >&2
    exit 2
fi

usage() {
    cat <<'EOF'
Install a prebuilt Termtrace release for the current Linux architecture.

Usage: install.sh [--version VERSION] [--install-dir DIRECTORY]

Environment:
  TERMTRACE_REPOSITORY     GitHub owner/repository (default: zigai/termtrace)
  TERMTRACE_VERSION        Release version or "latest"
  TERMTRACE_INSTALL_DIR    Destination directory (default: ~/.local/bin)
  TERMTRACE_DOWNLOAD_BASE  Override release URL for mirrors and tests
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            if [[ $# -lt 2 ]]; then
                echo "--version requires a value" >&2
                exit 2
            fi
            version=$2
            shift 2
            ;;
        --install-dir)
            if [[ $# -lt 2 ]]; then
                echo "--install-dir requires a value" >&2
                exit 2
            fi
            install_dir=$2
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ "$(uname -s)" != Linux ]]; then
    echo "Termtrace currently supports Linux only" >&2
    exit 2
fi

for musl_loader in /lib/ld-musl-*.so.1; do
    if [[ -e "$musl_loader" ]]; then
        echo "Termtrace prebuilt releases currently require glibc 2.28 or newer; musl is not yet supported" >&2
        exit 2
    fi
done

case "$(uname -m)" in
    x86_64 | amd64)
        target=x86_64-unknown-linux-gnu
        ;;
    aarch64 | arm64)
        target=aarch64-unknown-linux-gnu
        ;;
    *)
        echo "unsupported architecture: $(uname -m)" >&2
        exit 2
        ;;
esac

archive="termtrace-${target}.tar.gz"
checksum="${archive}.sha256"
work=$(mktemp -d)
candidate=

cleanup() {
    status=$?

    rm -rf "$work"

    if [[ -n "$candidate" ]]; then
        rm -f "$candidate"
    fi

    return "$status"
}

trap cleanup EXIT HUP INT TERM

download_with_curl() {
    local base=$1

    curl --fail --location --retry 3 --silent --show-error \
        "${base}/${archive}" \
        --output "${work}/${archive}" || return
    curl --fail --location --retry 3 --silent --show-error \
        "${base}/${checksum}" \
        --output "${work}/${checksum}"
}

download_release() {
    if [[ -n "$download_base" ]]; then
        download_with_curl "${download_base%/}"
        return
    fi

    local base tag
    if [[ "$version" == latest ]]; then
        base="https://github.com/${repository}/releases/latest/download"
    else
        case "$version" in
            v*)
                tag=$version
                ;;
            *)
                tag="v${version}"
                ;;
        esac
        base="https://github.com/${repository}/releases/download/${tag}"
    fi

    if download_with_curl "$base"; then
        return
    fi

    if ! command -v gh >/dev/null 2>&1; then
        echo "release download failed; private repositories require an authenticated gh command" >&2
        exit 1
    fi

    rm -f "${work}/${archive}" "${work}/${checksum}"

    if [[ "$version" == latest ]]; then
        gh release download \
            --repo "$repository" \
            --pattern "$archive" \
            --pattern "$checksum" \
            --dir "$work"
    else
        gh release download "$tag" \
            --repo "$repository" \
            --pattern "$archive" \
            --pattern "$checksum" \
            --dir "$work"
    fi
}

verify_checksum() {
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$work" && sha256sum --check "$checksum")
        return
    fi

    if command -v shasum >/dev/null 2>&1; then
        (cd "$work" && shasum -a 256 --check "$checksum")
        return
    fi

    echo "sha256sum or shasum is required to verify the release" >&2
    exit 2
}

download_release
verify_checksum
tar -xzf "${work}/${archive}" -C "$work"
binary="${work}/termtrace-${target}/termtrace"

if [[ ! -x "$binary" ]]; then
    echo "release archive contains no executable termtrace binary" >&2
    exit 1
fi

mkdir -p "$install_dir"
if [[ ! -w "$install_dir" ]]; then
    echo "install directory is not writable: $install_dir" >&2
    exit 1
fi

destination="${install_dir}/termtrace"
candidate=$(mktemp "${install_dir}/.termtrace.XXXXXX")
install -m 0555 "$binary" "$candidate"

"$candidate" doctor --require core,semantic >/dev/null

mv -f "$candidate" "$destination"
candidate=

printf 'Termtrace installed at %s\n' "$destination"

case ":${PATH}:" in
    *:"$install_dir":*)
        ;;
    *)
        printf 'Add %s to PATH, then run: termtrace doctor --explain\n' "$install_dir"
        ;;
esac
