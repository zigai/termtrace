//! Embeds the source revision used to build the test-fixture crate.

use std::{
    env,
    error::Error,
    io,
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TERMTRACE_BUILD_REVISION");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(io::Error::other("CARGO_MANIFEST_DIR is not set").into());
    };
    let manifest_dir = PathBuf::from(manifest_dir);
    watch_head(&manifest_dir);

    let build_revision = env::var("TERMTRACE_BUILD_REVISION")
        .ok()
        .filter(|value| return !value.trim().is_empty())
        .or_else(|| return git_output(&manifest_dir, &["rev-parse", "--short=12", "HEAD"]))
        .map_or_else(
            || return "unknown".to_owned(),
            |value| return value.trim().to_owned(),
        );

    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={build_revision}");

    return Ok(());
}

fn git_output(directory: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    return String::from_utf8(output.stdout).ok();
}

// In a worktree `.git` is a file pointing elsewhere, so git resolves each path. A commit on a
// branch leaves HEAD untouched but appends to its log. Cargo treats a missing path as always
// changed, so only existing files are watched.
fn watch_head(directory: &Path) {
    let arguments = ["rev-parse", "--git-path", "HEAD", "--git-path", "logs/HEAD"];
    let Some(paths) = git_output(directory, &arguments) else {
        return;
    };

    for path in paths.lines() {
        let path = directory.join(path);
        if path.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
