use super::finish_with_cleanup;
use crate::{
    analysis::profile::{
        builtin_terminal_colors, escape_config_string, resolve_profile, restore_profile,
        terminal_colors_from_config, ResolvedGhosttyProfile,
    },
    Error, Result,
};
use std::{
    env::temp_dir,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use termtrace_evidence::artifact::{create_private_directory, write_secure};
use termtrace_model::{
    execution::event::TerminalColors,
    execution::session::{GhosttyVisualOverrides, GhosttyVisualProvenance},
    value::ByteString,
};
use uuid::Uuid;

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
            query_isolated_config(wrapper.as_bytes())?,
            Some(ByteString::from(canonical.as_os_str().as_bytes())),
        )
    } else {
        (query_default_config()?, None)
    };

    return resolve_profile(&base, source_config, overrides, query_isolated_config);
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
    return restore_profile(replay_config, provenance, query_isolated_config);
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

fn materialize_profile(root: &Path, config: &[u8]) -> Result<PathBuf> {
    create_private_directory(root)?;
    let ghostty = root.join("ghostty");
    create_private_directory(&ghostty)?;
    write_secure(&ghostty.join("config.ghostty"), config)?;
    return Ok(root.to_path_buf());
}

fn query_default_config() -> Result<Vec<u8>> {
    let output = Command::new("ghostty")
        .arg("+show-config")
        .output()
        .map_err(|error| return profile_error(format!("run Ghostty config query: {error}")))?;

    return checked_config_output(output, "query default Ghostty config");
}

fn query_isolated_config(config: &[u8]) -> Result<Vec<u8>> {
    let root = temp_dir().join(format!("termtrace-ghostty-profile-{}", Uuid::now_v7()));
    let result = materialize_profile(&root, config).and_then(|_| {
        return Command::new("ghostty")
            .arg("+show-config")
            .env("XDG_CONFIG_HOME", &root)
            .output()
            .map_err(|error| {
                return profile_error(format!("run isolated Ghostty config query: {error}"));
            })
            .and_then(|output| return checked_config_output(output, "resolve Ghostty config"));
    });

    let cleanup = if root.exists() {
        fs_err::remove_dir_all(&root)
            .map_err(|source| return Error::Filesystem { path: root, source })
    } else {
        Ok(())
    };

    return finish_with_cleanup(result, cleanup);
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

fn profile_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "resolve Ghostty profile",
        reason: reason.into(),
    };
}
