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

    let ghostty_revision = fs::read_to_string(crate_root.join("ghostty-revision"))?
        .trim()
        .to_owned();

    check_submodule_pin(&repository_root, &ghostty_revision)?;

    let prefix = ghostty_prefix(&repository_root, &ghostty_revision)?;

    println!(
        "cargo:rerun-if-changed={}",
        prefix.join("include/ghostty/vt.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        prefix.join("lib/libghostty-vt.a").display()
    );
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

fn ghostty_prefix(repository_root: &Path, revision: &str) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(prefix) = env::var_os("TERMTRACE_GHOSTTY_PREFIX") {
        let prefix = PathBuf::from(prefix);
        if contains_ghostty_build(&prefix) {
            return Ok(prefix);
        }
        return Err(format!(
            "TERMTRACE_GHOSTTY_PREFIX does not contain a compatible libghostty-vt: {}",
            prefix.display()
        )
        .into());
    }

    let mut candidates = vec![repository_root.join("vendor/ghostty/zig-out")];

    if let Some(cache) = shared_cache(revision) {
        candidates.push(cache);
    }

    if let Some(prefix) = candidates
        .iter()
        .find(|candidate| return contains_ghostty_build(candidate))
    {
        return Ok(prefix.clone());
    }

    let searched = candidates
        .iter()
        .map(|candidate| return format!("  {}", candidate.display()))
        .collect::<Vec<_>>()
        .join("\n");

    return Err(format!(
        "pinned libghostty-vt {revision} is not ready\nrun `scripts/bootstrap.sh` in any Termtrace checkout\nsearched:\n{searched}"
    )
    .into());
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

fn contains_ghostty_build(prefix: &Path) -> bool {
    return prefix.join("include/ghostty/vt.h").is_file()
        && prefix.join("lib/libghostty-vt.a").is_file();
}
