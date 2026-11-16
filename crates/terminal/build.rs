//! Link the pinned libghostty-vt build, building it with Zig when no prebuilt one is found.

use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
};

const GHOSTTY_REPOSITORY: &str = "https://github.com/ghostty-org/ghostty.git";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TERMTRACE_GHOSTTY_PREFIX");
    println!("cargo:rerun-if-env-changed=XDG_CACHE_HOME");
    println!("cargo:rerun-if-env-changed=HOME");
    println!("cargo:rerun-if-changed=ghostty-revision");
    println!("cargo:rerun-if-changed=zig-version");

    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(io::Error::other("CARGO_MANIFEST_DIR is not set").into());
    };
    let crate_root = PathBuf::from(manifest_dir);
    let repository_root = crate_root.join("../..");

    println!("cargo:rerun-if-changed=src/ghostty_vt/bindings.rs");

    // An uninitialized submodule has no repository, and git would answer for this checkout.
    let ghostty = repository_root.join("vendor/ghostty");
    if ghostty.join(".git").exists() {
        watch_git_state(&ghostty, &["HEAD"]);
    }

    let ghostty_revision = fs::read_to_string(crate_root.join("ghostty-revision"))?
        .trim()
        .to_owned();

    println!("cargo:rustc-env=TERMTRACE_GHOSTTY_REVISION={ghostty_revision}");

    check_submodule_pin(&repository_root, &ghostty_revision)?;

    let prefix = ghostty_prefix(&crate_root, &repository_root, &ghostty_revision)?;

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

fn ghostty_prefix(
    crate_root: &Path,
    repository_root: &Path,
    revision: &str,
) -> Result<PathBuf, Box<dyn Error>> {
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

    let cache = shared_cache(revision);

    let mut candidates = vec![repository_root.join("vendor/ghostty/zig-out")];
    candidates.extend(cache.clone());
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

    let Some(cache) = cache else {
        return Err(format!(
            "pinned libghostty-vt {revision} is not ready, and neither XDG_CACHE_HOME nor HOME names a cache to build it in\nsearched:\n{}",
            searched.join("\n")
        )
        .into());
    };
    build_ghostty(crate_root, &cache, revision)?;
    watch_prefix(&cache);
    return Ok(cache);
}

fn build_ghostty(crate_root: &Path, cache: &Path, revision: &str) -> Result<(), Box<dyn Error>> {
    check_zig(crate_root)?;

    let Some(out_dir) = env::var_os("OUT_DIR") else {
        return Err(io::Error::other("OUT_DIR is not set").into());
    };
    let source = PathBuf::from(out_dir).join("ghostty");

    if source.exists() {
        fs::remove_dir_all(&source)?;
    }

    fs::create_dir_all(&source)?;

    run(Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&source))?;
    run(Command::new("git")
        .args([
            "fetch",
            "--quiet",
            "--depth",
            "1",
            GHOSTTY_REPOSITORY,
            revision,
        ])
        .current_dir(&source))?;
    run(Command::new("git")
        .args(["checkout", "--quiet", "--detach", "FETCH_HEAD"])
        .current_dir(&source))?;

    let Some(cache_parent) = cache.parent() else {
        return Err(format!("{} has no parent directory", cache.display()).into());
    };
    fs::create_dir_all(cache_parent)?;

    let staging = cache_parent.join(format!("{revision}.{}", process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }

    // Must match the default build of scripts/bootstrap-ghostty-vt.sh, which shares this cache.
    run(Command::new("zig")
        .args([
            "build",
            "-Demit-lib-vt=true",
            "-Doptimize=ReleaseFast",
            "--prefix",
        ])
        .arg(&staging)
        .current_dir(&source))?;

    fs::write(staging.join(REVISION_STAMP), format!("{revision}\n"))?;
    if matches!(prefix_build(cache, revision), PrefixBuild::Ready) {
        fs::remove_dir_all(&staging)?;
        return Ok(());
    }

    if cache.exists() {
        fs::remove_dir_all(cache)?;
    }

    fs::rename(&staging, cache)?;

    return Ok(());
}

// Ghostty's build accepts any patch release of its minimum Zig version from that patch on.
fn check_zig(crate_root: &Path) -> Result<(), Box<dyn Error>> {
    let required = fs::read_to_string(crate_root.join("zig-version"))?
        .trim()
        .to_owned();

    let found = match Command::new("zig").arg("version").output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8(output.stdout)?.trim().to_owned())
        }
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    let release = |version: &str| {
        let numbers = version
            .split(['-', '+'])
            .next()?
            .split('.')
            .map(|part| return part.parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()?;
        return match numbers.as_slice() {
            &[major, minor, patch] => Some((major, minor, patch)),
            _ => None,
        };
    };
    let Some((major, minor, patch)) = release(&required) else {
        return Err(format!("zig-version holds an invalid version: {required}").into());
    };
    if let Some(found_release) = found.as_deref().and_then(release)
        && found_release.0 == major
        && found_release.1 == minor
        && found_release.2 >= patch
    {
        return Ok(());
    }
    return Err(format!(
        "building libghostty-vt needs Zig {required} or a later {major}.{minor} patch release as `zig` on PATH, but {}; install it from https://ziglang.org/download/, or set TERMTRACE_GHOSTTY_PREFIX to a prebuilt libghostty-vt",
        found.map_or_else(
            || return "no working `zig` was found".to_owned(),
            |found| return format!("found Zig {found}"),
        ),
    )
    .into());
}

// Child output goes to stderr so that a line starting with `cargo:` is not read as a directive.
fn run(command: &mut Command) -> Result<(), Box<dyn Error>> {
    let status = command
        .stdout(Stdio::from(io::stderr()))
        .status()
        .map_err(|error| {
            return format!(
                "could not run {}: {error}",
                command.get_program().to_string_lossy()
            );
        })?;

    if !status.success() {
        return Err(format!("{command:?} failed with {status}").into());
    }
    return Ok(());
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
