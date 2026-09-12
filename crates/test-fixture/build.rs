//! Embeds the source revision used to build the test-fixture crate.

use std::{env, error::Error, io, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TERMTRACE_BUILD_REVISION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(io::Error::other("CARGO_MANIFEST_DIR is not set").into());
    };
    let manifest_dir = PathBuf::from(manifest_dir);

    let build_revision = env::var("TERMTRACE_BUILD_REVISION")
        .ok()
        .filter(|value| return !value.trim().is_empty())
        .or_else(|| {
            let output = Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .current_dir(&manifest_dir)
                .output()
                .ok()?;

            if !output.status.success() {
                return None;
            }
            return String::from_utf8(output.stdout).ok();
        })
        .map_or_else(
            || return "unknown".to_owned(),
            |value| return value.trim().to_owned(),
        );

    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={build_revision}");

    return Ok(());
}
