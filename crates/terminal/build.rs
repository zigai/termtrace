//! Link the pinned libghostty-vt build.

use std::{env, error::Error, io, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TERMTRACE_GHOSTTY_PREFIX");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(io::Error::other("CARGO_MANIFEST_DIR is not set").into());
    };
    let crate_root = PathBuf::from(manifest_dir);
    let repository_root = crate_root.join("../..");

    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join("vendor/ghostty/.git").display()
    );
    println!("cargo:rerun-if-changed=src/ghostty_vt/bindings.rs");
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/index").display()
    );

    let build_revision = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(&repository_root)
        .output()
        .ok()
        .filter(|output| return output.status.success())
        .and_then(|output| return String::from_utf8(output.stdout).ok())
        .map_or_else(
            || return "unknown".to_owned(),
            |value| return value.trim().to_owned(),
        );

    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={build_revision}");

    let external_prefix = env::var_os("TERMTRACE_GHOSTTY_PREFIX");
    let prefix = external_prefix.as_ref().map_or_else(
        || return repository_root.join("vendor/ghostty/zig-out"),
        PathBuf::from,
    );

    let header = prefix.join("include/ghostty/vt.h");
    let library = prefix.join("lib/libghostty-vt.a");

    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", library.display());

    if !header.is_file() || !library.is_file() {
        let message = if external_prefix.is_some() {
            format!(
                "TERMTRACE_GHOSTTY_PREFIX does not contain a compatible libghostty-vt\nexpected header: {}\nexpected library: {}",
                header.display(),
                library.display()
            )
        } else {
            format!(
                "pinned libghostty-vt is not ready\nrun `scripts/bootstrap.sh` from the repository root\nexpected header: {}\nexpected library: {}",
                header.display(),
                library.display()
            )
        };

        return Err(message.into());
    }

    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");

    return Ok(());
}
