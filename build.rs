//! Embed the source identity owned by the root package.

use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

struct Identity {
    version: String,
    commit: String,
    dirty: bool,
}

fn git(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)).into());
    }
    return Ok(String::from_utf8(output.stdout)?.trim().to_owned());
}

fn watch_checkout() -> Result<(), Box<dyn Error>> {
    let Ok(root) = git(&["rev-parse", "--show-toplevel"]) else {
        return Ok(());
    };
    for path in git(&["-C", &root, "ls-files"])?.lines() {
        println!("cargo:rerun-if-changed={root}/{path}");
    }

    for name in ["HEAD", "index", "refs", "packed-refs"] {
        println!(
            "cargo:rerun-if-changed={}",
            git(&["rev-parse", "--path-format=absolute", "--git-path", name])?
        );
    }

    return Ok(());
}

// A crates.io source has no Git checkout; Cargo records the packaged commit instead.
fn published_identity(vcs_info: &Path) -> Result<Identity, Box<dyn Error>> {
    let info: serde_json::Value = serde_json::from_str(&fs::read_to_string(vcs_info)?)?;
    let git = info
        .get("git")
        .ok_or_else(|| return format!("{} has no git entry", vcs_info.display()))?;

    let sha1 = git
        .get("sha1")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| return format!("{} has no git.sha1", vcs_info.display()))?;

    return Ok(Identity {
        version: env::var("CARGO_PKG_VERSION")?,
        commit: sha1.get(..12).unwrap_or(sha1).to_owned(),
        dirty: git
            .get("dirty")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    });
}

fn main() -> Result<(), Box<dyn Error>> {
    for name in [
        "TERMTRACE_BUILD_VERSION",
        "TERMTRACE_BUILD_REVISION",
        "TERMTRACE_BUILD_DIRTY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let vcs_info = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join(".cargo_vcs_info.json");
    let published = if vcs_info.is_file() {
        Some(published_identity(&vcs_info)?)
    } else {
        watch_checkout()?;
        None
    };

    let version = match (env::var("TERMTRACE_BUILD_VERSION"), &published) {
        (Ok(value), _) => value,
        (Err(_), Some(identity)) => identity.version.clone(),
        (Err(_), None) => git(&["describe", "--tags", "--dirty", "--always"])?,
    };

    let commit = match (env::var("TERMTRACE_BUILD_REVISION"), &published) {
        (Ok(value), _) => value,
        (Err(_), Some(identity)) => identity.commit.clone(),
        (Err(_), None) => git(&["rev-parse", "--short=12", "HEAD"])?,
    };

    let dirty = match (env::var("TERMTRACE_BUILD_DIRTY"), &published) {
        (Ok(value), _) => value.parse::<bool>()?,
        (Err(_), Some(identity)) => identity.dirty,
        (Err(_), None) => !git(&["diff", "--name-only", "HEAD"])?.is_empty(),
    };

    let suffix = if dirty { ", dirty" } else { "" };

    println!("cargo:rustc-env=TERMTRACE_BUILD_VERSION={version}");
    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={commit}");
    println!("cargo:rustc-env=TERMTRACE_BUILD_DIRTY_SUFFIX={suffix}");

    return Ok(());
}
