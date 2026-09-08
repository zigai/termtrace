//! Provide the source revision embedded in engine recording provenance.

use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=TERMTRACE_BUILD_REVISION");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION=unknown");

        return;
    };
    let manifest_dir = PathBuf::from(manifest_dir);
    let repository_root = manifest_dir.join("../..");
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repository_root.join(".git/index").display()
    );

    let build_revision = env::var("TERMTRACE_BUILD_REVISION")
        .ok()
        .filter(|value| return !value.trim().is_empty())
        .or_else(|| {
            let output = Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .current_dir(&repository_root)
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
}
