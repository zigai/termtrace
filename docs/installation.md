# Installation

## Supported host

The production capture path targets Linux because Ghostty's GTK application, Xvfb, X11 peer/process facilities, and `renameat2(RENAME_NOREPLACE)` are used directly. Semantic replay supports x86_64 and aarch64 Linux bootstrap archives.

## Runtime dependencies

- Rust 1.91.1 (selected by `rust-toolchain.toml`)
- Git with submodule support
- curl, SHA-256 utilities, and xz/tar
- installed Ghostty GTK build
- Xvfb
- FFmpeg and FFprobe with x11grab, PNG, and libx264rgb
- dbus-run-session
- util-linux `setsid` for detached named recordings
- OpenSSH `ssh-keygen` for optional detached evidence receipts
- jq for the native smoke script

## Build

```bash
git clone --recurse-submodules git@github.com:zigai/termtrace.git
cd termtrace
scripts/bootstrap.sh
cargo build --release
```

The bootstrap initializes a missing submodule, downloads Zig 0.15.2 over HTTPS, and verifies a platform-specific pinned SHA-256 before extraction. It builds the public Ghostty submodule at the exact revision recorded in Git and `replay::GHOSTTY_REVISION`. No generated Ghostty binary or Zig toolchain is committed. Rust bindings are checked in, so normal builds do not require bindgen, Clang, or libclang.

Maintainers upgrading the pinned Ghostty revision must install `bindgen-cli` 0.72.1 and run `scripts/generate-ghostty-vt-bindings.sh` after updating the revision constants. This tool is not an end-user or normal contributor dependency.

Run `cargo install --path .` after validation if a user-local binary is desired.
