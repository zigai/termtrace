use std::{
    env::temp_dir,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use termtrace_engine::{
    Error as EngineError, Result as EngineResult,
    analysis::profile::{ResolvedGhosttyProfile, resolve_profile, restore_profile},
};
use termtrace_evidence::artifact::{create_private_directory, write_secure};
use termtrace_model::{
    execution::session::{GhosttyVisualOverrides, GhosttyVisualProvenance},
    value::ByteString,
};

use super::{Error, Result};

pub(super) fn resolve_ghostty_profile(
    source_config: Option<&Path>,
    overrides: GhosttyVisualOverrides,
) -> Result<ResolvedGhosttyProfile> {
    let (base, source) = if let Some(source) = source_config {
        let canonical = fs::canonicalize(source).map_err(|source_error| {
            return Error::Filesystem {
                path: source.to_path_buf(),
                source: source_error,
            };
        })?;
        let path = canonical.to_str().ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "Ghostty config path must be valid UTF-8".to_owned(),
            };
        })?;
        let wrapper = format!(
            "config-file = \"{}\"\n",
            path.replace('\\', "\\\\").replace('"', "\\\"")
        );
        (
            query_isolated_config(wrapper.as_bytes())?,
            Some(ByteString::from(canonical.as_os_str().as_encoded_bytes())),
        )
    } else {
        (
            checked_output(
                Command::new("ghostty").arg("+show-config").output(),
                "query default Ghostty config",
            )?,
            None,
        )
    };
    return resolve_profile(&base, source, overrides, |config| {
        return query_isolated_config(config);
    })
    .map_err(Error::from);
}

pub(super) fn restore_ghostty_profile(
    replay_config: Vec<u8>,
    provenance: GhosttyVisualProvenance,
) -> Result<ResolvedGhosttyProfile> {
    return restore_profile(replay_config, provenance, |config| {
        return query_isolated_config(config);
    })
    .map_err(Error::from);
}

pub(super) fn materialize_profile(root: &Path, config: &[u8]) -> Result<PathBuf> {
    create_private_directory(root)?;
    let ghostty = root.join("ghostty");
    create_private_directory(&ghostty)?;
    write_secure(&ghostty.join("config.ghostty"), config)?;
    return Ok(root.to_path_buf());
}

fn query_isolated_config(config: &[u8]) -> EngineResult<Vec<u8>> {
    let root = temp_dir().join(format!(
        "termtrace-ghostty-profile-{}",
        uuid::Uuid::now_v7()
    ));
    let result = (|| {
        create_private_directory(&root).map_err(|error| {
            return EngineError::TerminalOperation {
                operation: "materialize Ghostty profile",
                reason: error.to_string(),
            };
        })?;
        let ghostty = root.join("ghostty");
        create_private_directory(&ghostty).map_err(|error| {
            return EngineError::TerminalOperation {
                operation: "materialize Ghostty profile",
                reason: error.to_string(),
            };
        })?;
        write_secure(&ghostty.join("config.ghostty"), config).map_err(|error| {
            return EngineError::TerminalOperation {
                operation: "materialize Ghostty profile",
                reason: error.to_string(),
            };
        })?;
        let output = Command::new("ghostty")
            .arg("+show-config")
            .env("XDG_CONFIG_HOME", &root)
            .output()
            .map_err(|error| {
                return EngineError::TerminalOperation {
                    operation: "resolve Ghostty profile",
                    reason: error.to_string(),
                };
            })?;
        if !output.status.success() {
            return Err(EngineError::TerminalOperation {
                operation: "resolve Ghostty profile",
                reason: format!(
                    "exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        return Ok(output.stdout);
    })();
    let cleanup = if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| {
            return EngineError::TerminalOperation {
                operation: "remove Ghostty profile",
                reason: error.to_string(),
            };
        })
    } else {
        Ok(())
    };
    match (result, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(error), _) | (Ok(_), Err(error)) => return Err(error),
    }
}

fn checked_output(output: io::Result<Output>, operation: &'static str) -> Result<Vec<u8>> {
    let output = output.map_err(|error| {
        return Error::ExternalCommand {
            operation: "ghostty".to_owned(),
            reason: format!("{operation}: {error}"),
        };
    })?;
    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: "ghostty".to_owned(),
            reason: format!(
                "{operation} exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    return Ok(output.stdout);
}
