//! Link the pinned libghostty-vt build.

use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TERMTRACE_GHOSTTY_PREFIX");
    println!("cargo:rerun-if-env-changed=XDG_CACHE_HOME");
    println!("cargo:rerun-if-env-changed=HOME");
    println!("cargo:rerun-if-changed=ghostty-revision");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(io::Error::other("CARGO_MANIFEST_DIR is not set").into());
    };
    let crate_root = PathBuf::from(manifest_dir);
    let repository_root = crate_root.join("../..");

    println!("cargo:rerun-if-changed=src/ghostty_vt/bindings.rs");
    watch_git_state(&repository_root, &["HEAD", "index", "packed-refs"]);
    watch_git_state(&repository_root.join("vendor/ghostty"), &["HEAD"]);

    let build_revision = git_output(&repository_root, &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| return "unknown".to_owned());

    println!("cargo:rustc-env=TERMTRACE_BUILD_REVISION={build_revision}");

    let ghostty_revision = fs::read_to_string(crate_root.join("ghostty-revision"))?
        .trim()
        .to_owned();

    println!("cargo:rustc-env=TERMTRACE_GHOSTTY_REVISION={ghostty_revision}");

    check_submodule_pin(&repository_root, &ghostty_revision)?;

    let prefix = ghostty_prefix(&repository_root, &ghostty_revision)?;

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

fn git_output(directory: &Path, arguments: &[&str]) -> Option<String> {
    return Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .ok()
        .filter(|output| return output.status.success())
        .and_then(|output| return String::from_utf8(output.stdout).ok())
        .map(|value| return value.trim().to_owned());
}

// In a worktree or submodule `.git` is a file pointing elsewhere, so git resolves each path;
// the current branch ref is watched too because a commit moves it without touching HEAD.
fn watch_git_state(directory: &Path, files: &[&str]) {
    let branch = git_output(directory, &["symbolic-ref", "-q", "HEAD"]);
    let mut arguments = vec!["rev-parse"];

    for file in files.iter().copied().chain(branch.as_deref()) {
        arguments.extend(["--git-path", file]);
    }

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

fn check_submodule_pin(repository_root: &Path, revision: &str) -> Result<(), Box<dyn Error>> {
    let Some(output) = Command::new("git")
        .args(["ls-files", "--stage", "--", "vendor/ghostty"])
        .current_dir(repository_root)
        .output()
        .ok()
        .filter(|output| return output.status.success())
    else {
        return Ok(());
    };
    let index_entry = String::from_utf8(output.stdout)?;
    let Some(pinned) = index_entry.split_whitespace().nth(1) else {
        return Ok(());
    };
    if pinned != revision {
        return Err(format!(
            "crates/terminal/ghostty-revision ({revision}) does not match the vendor/ghostty submodule pin ({pinned})"
        )
        .into());
    }
    return Ok(());
}

// The bootstrap writes the Ghostty revision it built next to the library, so a prefix left
// behind by an earlier submodule pin is never linked against the current bindings.
const REVISION_STAMP: &str = "ghostty-revision";

fn ghostty_prefix(repository_root: &Path, revision: &str) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(prefix) = env::var_os("TERMTRACE_GHOSTTY_PREFIX") {
        let prefix = PathBuf::from(prefix);
        watch_prefix(&prefix);

        return match prefix_build(&prefix, revision) {
            PrefixBuild::Ready => Ok(prefix),
            PrefixBuild::Missing => Err(format!(
                "TERMTRACE_GHOSTTY_PREFIX does not contain libghostty-vt: {}\nrun `scripts/bootstrap.sh` with the same TERMTRACE_GHOSTTY_PREFIX",
                prefix.display()
            )
            .into()),
            PrefixBuild::Stale(found) => Err(format!(
                "TERMTRACE_GHOSTTY_PREFIX {} {}, but crates/terminal/ghostty-revision pins {revision}\nrun `scripts/bootstrap.sh` with the same TERMTRACE_GHOSTTY_PREFIX",
                prefix.display(),
                found.as_deref().map_or_else(
                    || return "has no revision stamp".to_owned(),
                    |found| return format!("was built from Ghostty {found}"),
                ),
            )
            .into()),
        };
    }

    let mut candidates = vec![repository_root.join("vendor/ghostty/zig-out")];

    if let Some(cache) = shared_cache(revision) {
        candidates.push(cache);
    }

    let mut searched = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        watch_prefix(&candidate);

        match prefix_build(&candidate, revision) {
            PrefixBuild::Ready => return Ok(candidate),
            PrefixBuild::Missing => searched.push(format!("  {} (missing)", candidate.display())),
            PrefixBuild::Stale(found) => searched.push(format!(
                "  {} ({})",
                candidate.display(),
                found.map_or_else(
                    || return "no revision stamp".to_owned(),
                    |found| return format!("built from {found}"),
                ),
            )),
        }
    }

    return Err(format!(
        "pinned libghostty-vt {revision} is not ready\nrun `scripts/bootstrap.sh` in any Termtrace checkout\nsearched:\n{}",
        searched.join("\n")
    )
    .into());
}

enum PrefixBuild {
    Ready,
    Missing,
    Stale(Option<String>),
}

fn prefix_build(prefix: &Path, revision: &str) -> PrefixBuild {
    if !prefix.join("include/ghostty/vt.h").is_file()
        || !prefix.join("lib/libghostty-vt.a").is_file()
    {
        return PrefixBuild::Missing;
    }
    return match fs::read_to_string(prefix.join(REVISION_STAMP)) {
        Ok(stamp) if stamp.trim() == revision => PrefixBuild::Ready,
        Ok(stamp) => PrefixBuild::Stale(Some(stamp.trim().to_owned())),
        Err(_) => PrefixBuild::Stale(None),
    };
}

// Cargo treats a missing rerun-if-changed path as always changed, so only existing files are watched.
fn watch_prefix(prefix: &Path) {
    for file in [
        REVISION_STAMP,
        "include/ghostty/vt.h",
        "lib/libghostty-vt.a",
    ] {
        let path = prefix.join(file);
        if path.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn shared_cache(revision: &str) -> Option<PathBuf> {
    let cache_home = env::var_os("XDG_CACHE_HOME")
        .filter(|value| return !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            return env::var_os("HOME").map(|home| return PathBuf::from(home).join(".cache"));
        })?;

    return Some(cache_home.join("termtrace/libghostty-vt").join(revision));
}
