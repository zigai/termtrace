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

fn watch_checkout() -> Result<bool, Box<dyn Error>> {
    let Ok(root) = git(&["rev-parse", "--show-toplevel"]) else {
        return Ok(false);
    };
    // Cargo scans a watched directory recursively, and a tracked directory is a submodule
    // holding its own build output.
    for path in git(&["-C", &root, "ls-files"])?.lines() {
        let path = Path::new(&root).join(path);
        if !path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    // A commit on a branch leaves HEAD untouched but appends to its log, and tags change the
    // described version. Cargo treats a missing path as always changed.
    for name in ["HEAD", "logs/HEAD", "refs/tags", "packed-refs"] {
        let path = PathBuf::from(git(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            name,
        ])?);

        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    return Ok(true);
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
    // A source archive has neither a Git checkout nor Cargo's packaged commit.
    let recorded = if vcs_info.is_file() {
        Some(published_identity(&vcs_info)?)
    } else if watch_checkout()? {
        None
    } else {
        Some(Identity {
            version: env::var("CARGO_PKG_VERSION")?,
            commit: "unknown".to_owned(),
            dirty: false,
        })
    };

    let version = match (env::var("TERMTRACE_BUILD_VERSION"), &recorded) {
        (Ok(value), _) => value,
        (Err(_), Some(identity)) => identity.version.clone(),
        (Err(_), None) => git(&["describe", "--tags", "--dirty", "--always"])?,
    };

    let commit = match (env::var("TERMTRACE_BUILD_REVISION"), &recorded) {
        (Ok(value), _) => value,
        (Err(_), Some(identity)) => identity.commit.clone(),
        (Err(_), None) => git(&["rev-parse", "--short=12", "HEAD"])?,
    };

    let dirty = match (env::var("TERMTRACE_BUILD_DIRTY"), &recorded) {
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
