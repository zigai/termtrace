# Installation

Termtrace currently targets Linux. Recording and semantic replay work on x86_64 and aarch64; native capture requires additional GUI and media tools.

## Quick install

Install the latest prebuilt release without cloning the repository or installing Rust and Zig:

```bash
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
```

The high-level [`install.sh`](../install.sh) script detects the current architecture, verifies the archive's SHA-256 checksum, installs `termtrace` to `~/.local/bin`, installs managed Python and TypeScript assets under `~/.local/share/termtrace/clients`, and checks the core and semantic capabilities. It never invokes `sudo` or installs optional system packages. Because this command follows `main`, use the release-attached copy when you need a pinned installer:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/zigai/termtrace/releases/latest/download/termtrace-installer.sh | bash
```

Install a specific version or choose another destination with:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/zigai/termtrace/releases/latest/download/termtrace-installer.sh \
  | bash -s -- --version 0.1.0 --install-dir "$HOME/bin"
```

Rerun the installer to upgrade or replace an installation. Remove the installed binary to uninstall it.

### Private repository access

Release downloads from a private repository require an authenticated GitHub CLI. Download and run the installer with:

```bash
gh auth status
work=$(mktemp -d)
gh release download --repo zigai/termtrace --pattern termtrace-installer.sh --dir "$work"
bash "$work/termtrace-installer.sh"
rm -rf "$work"
```

The installer first attempts an ordinary release download, then uses the authenticated `gh` command when the repository is private.

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
termtrace doctor --require semantic-video,presentation
```

`semantic-video` requires the pinned semantic engine plus FFmpeg/FFprobe with `libx264rgb`; it does not require Ghostty GTK, Xvfb, D-Bus, ImageMagick, or OpenGL. Native video remains the default presentation source.

## Manual release archive

Tagged releases provide Linux core and semantic archives for x86_64 and aarch64. Release assets also include SHA-256 and BLAKE3 checksums, an SBOM, and GitHub build-provenance attestations. The binaries target glibc 2.28 or newer.

Download the archive and matching `.sha256` file, then verify and install it:

```bash
sha256sum --check termtrace-TARGET.tar.gz.sha256
tar -xzf termtrace-TARGET.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 0555 termtrace-TARGET/termtrace "$HOME/.local/bin/termtrace"
mkdir -p "$HOME/.local/share/termtrace"
cp -R termtrace-TARGET/clients "$HOME/.local/share/termtrace/clients"
termtrace doctor --require core,semantic
termtrace client status
```

The repository's `scripts/validate-release-archive.sh` performs a deeper recording, content-verification, and semantic-replay smoke test. Run `doctor` on the destination machine to establish native capabilities. GitHub documents independent attestation verification in [Verifying the integrity of a release](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/secure-your-dependencies/verify-release-integrity).

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

`cargo install` installs only the executable. While developing from a clone, the executable discovers `packages/` automatically; set `TERMTRACE_CLIENT_ASSETS="$PWD/packages"` explicitly if it is moved elsewhere. Release archives and the installer carry the built client assets with the binary.

Set `TERMTRACE_GHOSTTY_PREFIX` only when intentionally testing a compatible external semantic-library build.

## Source build requirements

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
