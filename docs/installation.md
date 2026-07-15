# Installation

Termtrace currently targets Linux. Recording and semantic replay work on x86_64 and aarch64; native capture requires additional GUI and media tools.

## Check capabilities

After installing, run:

```bash
termtrace doctor --explain
```

Capabilities are reported independently. A machine may support recording and semantic replay without supporting native screenshots or video.

Use `--require` in CI:

```bash
termtrace doctor --require core,semantic
termtrace doctor --require native-screenshot,presentation
```

## Release archives

Tagged releases provide Linux core and semantic archives for x86_64 and aarch64. Each archive includes checksums, an SBOM, provenance metadata, and cargo-binstall metadata.

Verify a downloaded archive before installation:

```bash
sha256sum --check termtrace-TARGET.tar.zst.sha256
b3sum --check termtrace-TARGET.tar.zst.blake3
scripts/validate-release-archive.sh termtrace-TARGET.tar.zst
```

The validation script tests recording, content verification, and semantic replay. Run `doctor` on the destination machine to establish native capabilities.

## Build from source

Clone with submodules, then run the bootstrap:

```bash
git clone --recurse-submodules git@github.com:zigai/termtrace.git
cd termtrace
scripts/bootstrap.sh
cargo build --release
```

The bootstrap downloads the pinned Zig toolchain, verifies its SHA-256 checksum, and builds the pinned semantic library. Rust bindings are checked in, so normal builds do not require bindgen, Clang, or libclang.

Install the validated binary for the current user with:

```bash
cargo install --path .
```

Set `TERMTRACE_GHOSTTY_PREFIX` only when intentionally testing a compatible external semantic-library build.

## Core requirements

- Rust 1.91.1, selected by `rust-toolchain.toml`
- Git with submodule support
- curl, tar, xz, and SHA-256 utilities

Detached named recordings also require util-linux `setsid`. Detached evidence signing requires OpenSSH `ssh-keygen`.

## Native capture requirements

Native screenshots and video use the installed GTK terminal application on Xvfb. The host needs:

- Ghostty GTK;
- Xvfb and D-Bus session support;
- ImageMagick `import` for screenshots;
- FFmpeg and FFprobe with `x11grab`, PNG, and `libx264rgb` support.

`doctor` probes the actual launch and codec path. Tool presence alone is not considered readiness.

An optional container definition is available at `packaging/native/Containerfile`. Build it with an OCI base pinned by digest:

```bash
scripts/build-native-image.sh termtrace/native:VERSION ubuntu@sha256:EXACT_DIGEST
```

Pixels produced there describe that container environment, not every workstation.

## Shell completions

Generate completions from the installed command graph:

```bash
termtrace completions bash
termtrace completions zsh
termtrace completions fish
```

Install or source the output using your shell's normal mechanism. Regenerate it after upgrading.

## Maintainer note

Updating the pinned semantic dependency requires `bindgen-cli` 0.72.1 and `scripts/generate-ghostty-vt-bindings.sh`. This is not part of a normal build.
