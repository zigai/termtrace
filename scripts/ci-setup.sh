#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly mode="${1:?usage: ci-setup.sh core|native}"
case "$mode" in
    core | native) ;;
    *)
        printf 'unknown CI runtime: %s\n' "$mode" >&2
        exit 2
        ;;
esac

# Ubuntu 24.04 is the CI system-package baseline; language dependencies use
# the committed locks, and Ghostty uses the repository's pinned submodule.
sudo apt-get update
packages=(build-essential pkg-config curl git xz-utils python3 ffmpeg jq ripgrep
    ncurses-bin ncurses-term fonts-dejavu-core time)
if [[ "$mode" == native ]]; then
    packages+=(libgtk-4-dev libadwaita-1-dev libx11-dev libfontconfig1-dev
        libfreetype-dev libgl1-mesa-dri libegl-mesa0 gettext meson ninja-build
        python3-gi gir1.2-gtk-4.0 gir1.2-adw-1 xvfb xauth x11-utils
        dbus-daemon dbus-x11 imagemagick tmux)
fi
sudo apt-get install --yes --no-install-recommends "${packages[@]}"

"$root/scripts/bootstrap.sh"

if [[ "$mode" == native ]]; then
    # Noble's Blueprint 0.12 is older than this Ghostty checkout's 0.16 minimum.
    readonly blueprint_commit=04ef0944db56ab01307a29aaa7303df6067cb3c0
    readonly blueprint="$root/.tools/blueprint-$blueprint_commit"
    if [[ ! -d "$blueprint/.git" ]]; then
        git init "$blueprint"
        git -C "$blueprint" remote add origin https://gitlab.gnome.org/GNOME/blueprint-compiler.git
    fi
    git -C "$blueprint" fetch --depth 1 origin "$blueprint_commit"
    git -C "$blueprint" checkout --detach "$blueprint_commit"
    meson setup --reconfigure "$blueprint/build" "$blueprint" --prefix "$root/.tools/ci" -Ddocs=false
    meson install -C "$blueprint/build"
    export PATH="$root/.tools/ci/bin:$PATH"
    readonly zig="$root/.tools/zig-$(uname -m)-linux-0.17.0/zig"
    (
        cd "$root/vendor/ghostty"
        "$zig" build -Doptimize=ReleaseFast -Dapp-runtime=gtk \
            -Dgtk-x11=true -Dgtk-wayland=false -Demit-docs=false -Demit-terminfo=true
    )
    test -x "$root/vendor/ghostty/zig-out/bin/ghostty"
fi

# The same environment is consumed locally by ci-run.sh and by Actions.
mkdir -p "$root/.tools"
{
    printf 'export TERM=%q\n' xterm-256color
    printf 'export LANG=%q\n' C.UTF-8
    printf 'export LC_ALL=%q\n' C.UTF-8
    printf 'export LIBGL_ALWAYS_SOFTWARE=%q\n' 1
    if [[ "$mode" == native ]]; then
        printf 'export PATH=%q:"$PATH"\n' "$root/vendor/ghostty/zig-out/bin:$root/.tools/ci/bin"
        printf 'export TERMINFO_DIRS=%q\n' "$root/vendor/ghostty/zig-out/share/terminfo:/usr/share/terminfo:/lib/terminfo"
    fi
} >"$root/.tools/ci-env.sh"
if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf 'TERM=xterm-256color\nLANG=C.UTF-8\nLC_ALL=C.UTF-8\nLIBGL_ALWAYS_SOFTWARE=1\n' >>"$GITHUB_ENV"
    if [[ "$mode" == native ]]; then
        printf '%s\n' "$root/vendor/ghostty/zig-out/bin" "$root/.tools/ci/bin" >>"$GITHUB_PATH"
        printf 'TERMINFO_DIRS=%s\n' "$root/vendor/ghostty/zig-out/share/terminfo:/usr/share/terminfo:/lib/terminfo" >>"$GITHUB_ENV"
    fi
fi
