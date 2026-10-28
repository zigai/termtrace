use super::finish_with_cleanup;
use crate::{
    analysis::profile::{
        builtin_terminal_colors, escape_config_string, resolve, restore,
        terminal_colors_from_config, ResolvedGhosttyProfile,
    },
    Error, Result,
};
use std::{
    env::temp_dir,
    ffi::OsStr,
    fs::File,
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    result::Result as StdResult,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use termtrace_evidence::artifact::{create_private_directory, write_secure};
use termtrace_model::{
    execution::event::TerminalColors,
    execution::session::{GhosttyVisualOverrides, GhosttyVisualProvenance},
    execution::visual_profile::{VisualProfile, VisualProfileFailure},
    value::ByteString,
};
use termtrace_process::{find_executable, poll_until, SupervisedChild};
use uuid::Uuid;

static NOT_CANCELLED: AtomicBool = AtomicBool::new(false);
const GHOSTTY_QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const GHOSTTY_QUERY_POLL: Duration = Duration::from_millis(10);
const MAX_QUERY_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REPORTED_DIAGNOSTICS: usize = 8;
const FONT_FAMILY_KEYS: [&str; 4] = [
    "font-family",
    "font-family-bold",
    "font-family-italic",
    "font-family-bold-italic",
];

/// A native visual profile resolved by the installed Ghostty and checked for exact fonts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProfileResolution {
    /// Canonical replay and effective configuration with split hashes.
    pub profile: ResolvedGhosttyProfile,
    /// Installed Ghostty version report, first line.
    pub ghostty_version: String,
    /// BLAKE3 of the Ghostty executable that resolved the profile.
    pub ghostty_executable_blake3: String,
    /// Font families named by the effective configuration.
    pub requested_font_families: Vec<String>,
    /// Requested families Ghostty discovered under their exact names.
    pub discovered_font_families: Vec<String>,
}

/// Validate a visual profile and resolve its native section, when one is selected.
///
/// A semantic-only profile is validated without running any native program. A native profile
/// starts from the supplied config or an isolated Ghostty-default base that never reads the
/// user's config, applies its focused fields last, and must pass `ghostty +validate-config`
/// for both the source and the resolved bytes. Every font family named by the effective
/// configuration must be installed under its exact name; no substitute is accepted. Setting
/// `cancelled` stops the running Ghostty query and starts no further one.
///
/// # Errors
///
/// Returns [`Error::VisualProfile`] classifying an invalid profile, missing Ghostty, config
/// diagnostics, a missing font family, or a Ghostty query that failed, timed out, or was
/// cancelled.
pub fn resolve_visual_profile(
    profile: &VisualProfile,
    cancelled: &AtomicBool,
) -> Result<Option<NativeProfileResolution>> {
    let normalized = profile.normalized().map_err(|error| {
        return visual_error(
            VisualProfileFailure::InvalidVisualProfile,
            error.to_string(),
        );
    })?;

    let Some(native) = normalized.native else {
        return Ok(None);
    };

    let executable = ghostty_executable()?;
    let executable_bytes = fs_err::read(&executable).map_err(|source| {
        return visual_error(
            VisualProfileFailure::NativeProfileUnavailable,
            format!("read Ghostty executable: {source}"),
        );
    })?;

    let ghostty_executable_blake3 = blake3::hash(&executable_bytes).to_hex().to_string();
    let version = native_query(
        Command::new(&executable).arg("+version"),
        "query Ghostty version",
        cancelled,
    )?;

    let ghostty_version = String::from_utf8_lossy(&version)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();

    let (base, source_config) = if let Some(config) = &native.config {
        let path = Path::new(config);
        validate_config_file(&executable, path, cancelled)?;
        let wrapper = format!("config-file = \"{}\"\n", escape_config_string(config));

        (
            query_isolated_config(wrapper.as_bytes(), cancelled).map_err(native_failure)?,
            Some(ByteString::from(path.as_os_str().as_bytes())),
        )
    } else {
        (Vec::new(), None)
    };

    let resolved = resolve(&base, source_config, native.overrides(), |config| {
        return query_isolated_config(config, cancelled);
    })
    .map_err(native_failure)?;

    with_isolated_config(&resolved.replay_config, |root| {
        return validate_config_file(
            &executable,
            &root.join("ghostty").join("config.ghostty"),
            cancelled,
        );
    })?;

    let requested_font_families = requested_font_families(&resolved.effective_config);
    let discovered_font_families = require_font_families(
        &executable,
        &resolved.replay_config,
        &requested_font_families,
        cancelled,
    )?;

    return Ok(Some(NativeProfileResolution {
        profile: resolved,
        ghostty_version,
        ghostty_executable_blake3,
        requested_font_families,
        discovered_font_families,
    }));
}

/// Require every font family named by a recorded effective configuration to be installed under
/// its exact name, so native output never renders a profile with substituted fonts.
///
/// # Errors
///
/// Returns [`Error::VisualProfile`] with `font_unavailable` for a missing family, or
/// `native_profile_unavailable` when Ghostty cannot be found or queried.
pub fn verify_profile_fonts(replay_config: &[u8], effective_config: &[u8]) -> Result<()> {
    require_font_families(
        &ghostty_executable()?,
        replay_config,
        &requested_font_families(effective_config),
        &NOT_CANCELLED,
    )?;

    return Ok(());
}

/// Resolve a custom Ghostty config and focused overrides into one canonical profile.
///
/// A supplied config file replaces Ghostty's default user config. Without one,
/// the current effective user config is the base. Convenience overrides are
/// applied last and therefore always win.
///
/// # Errors
///
/// Returns an error when Ghostty cannot read the source, an override is invalid,
/// or the private temporary profile cannot be persisted.
pub fn resolve_ghostty_profile(
    source_config: Option<&Path>,
    overrides: GhosttyVisualOverrides,
) -> Result<ResolvedGhosttyProfile> {
    let (base, source_config) = if let Some(source) = source_config {
        let canonical = fs_err::canonicalize(source).map_err(|source_error| {
            return Error::Filesystem {
                path: source.to_path_buf(),
                source: source_error,
            };
        })?;

        let Some(path) = canonical.to_str() else {
            return Err(profile_error("Ghostty config path must be valid UTF-8"));
        };
        let wrapper = format!("config-file = \"{}\"\n", escape_config_string(path));

        (
            query_isolated_config(wrapper.as_bytes(), &NOT_CANCELLED)?,
            Some(ByteString::from(canonical.as_os_str().as_bytes())),
        )
    } else {
        (query_default_config()?, None)
    };

    return resolve(&base, source_config, overrides, |config| {
        return query_isolated_config(config, &NOT_CANCELLED);
    });
}

/// Restore and validate a profile persisted in a canonical recording.
///
/// # Errors
///
/// Returns an error when either persisted hash is invalid or Ghostty cannot
/// resolve the replay profile in isolation.
pub fn restore_ghostty_profile(
    replay_config: Vec<u8>,
    provenance: GhosttyVisualProvenance,
) -> Result<ResolvedGhosttyProfile> {
    return restore(replay_config, provenance, |config| {
        return query_isolated_config(config, &NOT_CANCELLED);
    });
}

/// Read ambient Ghostty colors, falling back only when the ambient probe fails.
///
/// # Errors
///
/// Returns an error when the pinned semantic engine cannot expose defaults or a
/// successful Ghostty query contains malformed explicit colors.
pub fn resolved_terminal_colors() -> Result<TerminalColors> {
    let colors = builtin_terminal_colors()?;
    let output = match Command::new("ghostty").arg("+show-config").output() {
        Ok(output) => output,
        Err(error) => {
            tracing::debug!(%error, "Ghostty is unavailable; recording pinned libghostty default colors");

            return Ok(colors);
        }
    };

    if !output.status.success() {
        tracing::debug!(status = %output.status, "Ghostty config query failed; recording pinned libghostty default colors");

        return Ok(colors);
    }
    return terminal_colors_from_config(&output.stdout);
}

fn materialize(root: &Path, config: &[u8]) -> Result<PathBuf> {
    create_private_directory(root)?;
    let ghostty = root.join("ghostty");
    create_private_directory(&ghostty)?;
    write_secure(&ghostty.join("config.ghostty"), config)?;

    return Ok(root.to_path_buf());
}

fn query_default_config() -> Result<Vec<u8>> {
    let output = run_bounded(Command::new("ghostty").arg("+show-config"), &NOT_CANCELLED)
        .map_err(|error| return profile_error(format!("run Ghostty config query: {error}")))?;

    return checked_config_output(output, "query default Ghostty config");
}

fn query_isolated_config(config: &[u8], cancelled: &AtomicBool) -> Result<Vec<u8>> {
    return with_isolated_config(config, |root| {
        let output = run_bounded(
            Command::new("ghostty")
                .arg("+show-config")
                .env("XDG_CONFIG_HOME", root),
            cancelled,
        )
        .map_err(|error| {
            return profile_error(format!("run isolated Ghostty config query: {error}"));
        })?;

        return checked_config_output(output, "resolve Ghostty config");
    });
}

fn with_isolated_config<T>(config: &[u8], run: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let root = temp_dir().join(format!("termtrace-ghostty-profile-{}", Uuid::now_v7()));
    let result = materialize(&root, config).and_then(|root| return run(&root));

    let cleanup = if root.exists() {
        fs_err::remove_dir_all(&root)
            .map_err(|source| return Error::Filesystem { path: root, source })
    } else {
        Ok(())
    };

    return finish_with_cleanup(result, cleanup);
}

fn ghostty_executable() -> Result<PathBuf> {
    return find_executable("ghostty").ok_or_else(|| {
        return visual_error(
            VisualProfileFailure::NativeProfileUnavailable,
            "native visual profile requires Ghostty on PATH",
        );
    });
}

fn require_font_families(
    executable: &Path,
    replay_config: &[u8],
    families: &[String],
    cancelled: &AtomicBool,
) -> Result<Vec<String>> {
    let mut discovered = Vec::new();
    for family in families {
        let listing = with_isolated_config(replay_config, |root| {
            let mut argument = OsStr::new("--family=").to_os_string();
            argument.push(family);

            return native_query(
                Command::new(executable)
                    .arg("+list-fonts")
                    .arg(argument)
                    .env("XDG_CONFIG_HOME", root),
                "list Ghostty fonts",
                cancelled,
            );
        })?;

        let offered = listed_font_families(&listing);
        if !offered.iter().any(|candidate| return candidate == family) {
            return Err(visual_error(
                VisualProfileFailure::FontUnavailable,
                format!(
                    "font family `{family}` is not installed; Ghostty offered {}",
                    offered
                        .iter()
                        .take(3)
                        .map(|candidate| return format!("`{candidate}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        discovered.push(family.clone());
    }

    return Ok(discovered);
}

fn validate_config_file(executable: &Path, config: &Path, cancelled: &AtomicBool) -> Result<()> {
    let mut argument = OsStr::new("--config-file=").to_os_string();
    argument.push(config);
    let output = run_bounded(
        Command::new(executable)
            .arg("+validate-config")
            .arg(argument),
        cancelled,
    )
    .map_err(|error| {
        return visual_error(
            VisualProfileFailure::NativeProfileUnavailable,
            format!("run Ghostty config validation: {error}"),
        );
    })?;

    if output.status.success() {
        return Ok(());
    }

    let diagnostics = String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
        .map(|line| {
            // Diagnostics start with the config path; report only the line and key context.
            return line
                .strip_prefix(config.to_string_lossy().as_ref())
                .unwrap_or(line)
                .trim()
                .to_owned();
        })
        .filter(|line| return !line.is_empty())
        .take(MAX_REPORTED_DIAGNOSTICS)
        .collect::<Vec<_>>()
        .join("; ");

    return Err(visual_error(
        VisualProfileFailure::GhosttyConfigInvalid,
        format!(
            "Ghostty rejected the configuration ({}): {diagnostics}",
            output.status
        ),
    ));
}

fn native_query(
    command: &mut Command,
    operation: &'static str,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>> {
    let output = run_bounded(command, cancelled).map_err(|error| {
        return visual_error(
            VisualProfileFailure::NativeProfileUnavailable,
            format!("{operation}: {error}"),
        );
    })?;

    if !output.status.success() {
        return Err(visual_error(
            VisualProfileFailure::NativeProfileUnavailable,
            format!(
                "{operation} exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    return Ok(output.stdout);
}

// Output goes to private files because Ghostty can block on a full pipe that nothing drains.
fn run_bounded(command: &mut Command, cancelled: &AtomicBool) -> StdResult<Output, String> {
    return run_bounded_for(command, GHOSTTY_QUERY_TIMEOUT, cancelled);
}

fn run_bounded_for(
    command: &mut Command,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> StdResult<Output, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("cancelled".to_owned());
    }

    let root = temp_dir().join(format!("termtrace-ghostty-query-{}", Uuid::now_v7()));
    create_private_directory(&root).map_err(|error| return error.to_string())?;
    let result = run_bounded_in(command, &root, timeout, cancelled);
    let cleanup = fs_err::remove_dir_all(&root);

    return match (result, cleanup) {
        (Ok(output), Ok(())) => Ok(output),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(format!("remove query output: {error}")),
    };
}

fn run_bounded_in(
    command: &mut Command,
    root: &Path,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> StdResult<Output, String> {
    let stdout_path = root.join("stdout");
    let stderr_path = root.join("stderr");
    let create = |path: &Path| {
        return File::create_new(path).map_err(|error| return error.to_string());
    };
    let mut child = SupervisedChild::spawn(
        command
            .stdin(Stdio::null())
            .stdout(create(&stdout_path)?)
            .stderr(create(&stderr_path)?),
    )
    .map_err(|error| return error.to_string())?;

    let waited = poll_until(Instant::now() + timeout, GHOSTTY_QUERY_POLL, || {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".to_owned());
        }
        return child.try_wait().map_err(|error| return error.to_string());
    });

    let status = match waited {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _killed = child.kill();

            return Err(format!("did not finish within {} ms", timeout.as_millis()));
        }
        Err(error) => {
            let _killed = child.kill();

            return Err(error);
        }
    };

    let read = |path: &Path| {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| return file.take(MAX_QUERY_OUTPUT_BYTES).read_to_end(&mut bytes))
            .map_err(|error| return error.to_string())?;

        return Ok::<_, String>(bytes);
    };

    return Ok(Output {
        status,
        stdout: read(&stdout_path)?,
        stderr: read(&stderr_path)?,
    });
}

fn requested_font_families(effective_config: &[u8]) -> Vec<String> {
    let mut families: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(effective_config).lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();

        if FONT_FAMILY_KEYS.contains(&name.trim())
            && !value.is_empty()
            && !families.iter().any(|family| return family == value)
        {
            families.push(value.to_owned());
        }
    }

    return families;
}

fn listed_font_families(listing: &[u8]) -> Vec<String> {
    return String::from_utf8_lossy(listing)
        .lines()
        .filter(|line| return !line.is_empty() && !line.starts_with(char::is_whitespace))
        .map(|line| return line.trim_end().to_owned())
        .collect();
}

fn checked_config_output(output: Output, operation: &'static str) -> Result<Vec<u8>> {
    if !output.status.success() {
        return Err(profile_error(format!(
            "{operation} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    return Ok(output.stdout);
}

fn native_failure(error: Error) -> Error {
    if matches!(error, Error::VisualProfile { .. }) {
        return error;
    }
    return visual_error(VisualProfileFailure::NativeProfileFailed, error.to_string());
}

fn visual_error(failure: VisualProfileFailure, reason: impl Into<String>) -> Error {
    return Error::VisualProfile {
        failure,
        reason: reason.into(),
    };
}

fn profile_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "resolve Ghostty profile",
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn requested_families_cover_every_explicit_style_once() {
        let effective = b"font-family = \nfont-family = Example Mono\nfont-family-bold = Example Mono\nfont-family-italic = Example Italic\nfont-family-bold-italic = \nfont-size = 15\n";
        assert_eq!(
            requested_font_families(effective),
            vec!["Example Mono".to_owned(), "Example Italic".to_owned()]
        );
        assert!(requested_font_families(b"font-family = \nfont-size = 12\n").is_empty());
    }

    #[test]
    fn listed_families_exclude_face_lines_and_alternatives_do_not_match() {
        let listing = b"Fallback Mono\n  Fallback Mono Regular\n  Example Mono Regular\nOther Sans\n  Other Sans Bold\n";
        let families = listed_font_families(listing);
        assert_eq!(
            families,
            vec!["Fallback Mono".to_owned(), "Other Sans".to_owned()]
        );
        assert!(!families
            .iter()
            .any(|family| return family == "Example Mono"));
    }

    #[test]
    fn bounded_queries_kill_programs_that_do_not_finish() {
        let started = Instant::now();
        let output = run_bounded_for(
            Command::new("/bin/sh").args(["-c", "exec sleep 600"]),
            Duration::from_millis(200),
            &NOT_CANCELLED,
        );
        assert!(output.is_err_and(|error| return error.contains("did not finish")));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancellation_stops_a_running_query_and_starts_no_other() {
        let cancelled = AtomicBool::new(false);
        let started = Instant::now();
        let output = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(200));

                cancelled.store(true, Ordering::Release);
            });

            return run_bounded_for(
                Command::new("/bin/sh").args(["-c", "exec sleep 600"]),
                Duration::from_secs(60),
                &cancelled,
            );
        });
        assert!(output.is_err_and(|error| return error == "cancelled"));
        assert!(started.elapsed() < Duration::from_secs(5));

        let directory = tempfile::tempdir().expect("create marker directory");
        let marker = directory.path().join("spawned");
        let output = run_bounded_for(
            Command::new("/bin/sh")
                .arg("-c")
                .arg("touch \"$0\"")
                .arg(&marker),
            Duration::from_secs(5),
            &cancelled,
        );
        assert!(output.is_err_and(|error| return error == "cancelled"));
        assert!(!marker.exists());
    }
}
