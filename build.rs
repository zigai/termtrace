//! Embed the source identity owned by the root package.

use std::{env, error::Error, io, process::Command};

fn git(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)).into());
    }
    return Ok(String::from_utf8(output.stdout)?.trim().to_owned());
}

fn main() -> Result<(), Box<dyn Error>> {
    for name in [
        "TERMTRACE_BUILD_VERSION",
        "TERMTRACE_BUILD_REVISION",
        "TERMTRACE_BUILD_DIRTY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let version = match env::var("TERMTRACE_BUILD_VERSION") {
        Ok(value) => value,
        Err(_) => git(&["describe", "--tags", "--dirty", "--always"])?,
    };

    let commit = match env::var("TERMTRACE_BUILD_REVISION") {
        Ok(value) => value,
        Err(_) => git(&["rev-parse", "--short=12", "HEAD"])?,
    };

    let dirty = match env::var("TERMTRACE_BUILD_DIRTY") {
        Ok(value) => value.parse::<bool>()?,
        Err(_) => !git(&["diff", "--name-only", "HEAD"])?.is_empty(),
    };

    if let Ok(root) = git(&["rev-parse", "--show-toplevel"]) {
        for path in git(&["-C", &root, "ls-files"])?.lines() {
            println!("cargo:rerun-if-changed={root}/{path}");
        }

        for name in ["HEAD", "index", "refs", "packed-refs"] {
            println!(
                "cargo:rerun-if-changed={}",
                git(&["rev-parse", "--path-format=absolute", "--git-path", name])?
            );
        }
    }

    let suffix = if dirty { ", dirty" } else { "" };

    println!("cargo:rustc-env=TERMTRACE_BUILD_VERSION={version}");
    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={commit}");
    println!("cargo:rustc-env=TERMTRACE_BUILD_DIRTY_SUFFIX={suffix}");

    return Ok(());
}
