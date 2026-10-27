use super::client_error;
use crate::{Error, Result};
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    str,
};
use termtrace_control::observe_phase;
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::execution::session::SessionPhase;

/// A session named by its canonical directory or by a registry alias.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTarget {
    /// Canonical session directory.
    Path(PathBuf),
    /// Registry alias.
    Name(String),
}

/// Where a target lives and the phase observed there, without requiring it to be healthy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedSession {
    /// Canonical session directory, or the pending directory of an alias reservation.
    pub path: PathBuf,
    /// Observed session phase.
    pub phase: SessionPhase,
    /// Why the session is not simply live or completed, when known.
    pub diagnostic: Option<String>,
}

impl SessionTarget {
    /// Classify a reference: `path:` and `name:` prefixes are explicit; otherwise text with a
    /// `/`, a leading `.`, or an existing entry is a path and anything else is an alias.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty `path:` reference or an alias that is not UTF-8.
    pub fn parse(reference: &OsStr) -> Result<Self> {
        let bytes = reference.as_bytes();
        if let Some(path) = bytes.strip_prefix(b"path:") {
            if path.is_empty() {
                return Err(client_error("path: session reference is empty"));
            }
            return Ok(Self::Path(PathBuf::from(OsString::from_vec(path.to_vec()))));
        }

        let name = if let Some(name) = bytes.strip_prefix(b"name:") {
            name
        } else if bytes.contains(&b'/')
            || bytes.starts_with(b".")
            || PathBuf::from(reference).exists()
        {
            return Ok(Self::Path(PathBuf::from(reference)));
        } else {
            bytes
        };

        return str::from_utf8(name)
            .map(|name| return Self::Name(name.to_owned()))
            .map_err(|source| {
                return Error::InvalidOperation {
                    reason: "session aliases must be UTF-8; use path: for other paths".to_owned(),
                    source: Some(Box::new(source)),
                };
            });
    }

    /// Resolve to a healthy canonical session directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the alias is unknown, pending, or bound to changed evidence.
    pub fn path(&self) -> Result<PathBuf> {
        return self.path_in(None);
    }

    /// Resolve to a healthy canonical session directory, reading aliases from the registry
    /// under `state_root`, or the default registry when it is `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when the alias is unknown, pending, or bound to changed evidence.
    pub fn path_in(&self, state_root: Option<&Path>) -> Result<PathBuf> {
        return match self {
            Self::Path(path) => Ok(path.clone()),
            Self::Name(name) => {
                let registry = match state_root {
                    Some(root) => SessionRegistry::open(root)?,
                    None => SessionRegistry::open_default()?,
                };

                Ok(registry.resolve(name)?.session_path())
            }
        };
    }

    /// Locate the target and observe its phase, reporting unhealthy aliases as a phase.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry record itself is missing, insecure, or malformed.
    pub fn locate(&self) -> Result<LocatedSession> {
        return match self {
            Self::Path(path) => {
                let observation = observe_phase(path);

                Ok(LocatedSession {
                    path: path.clone(),
                    phase: observation.phase,
                    diagnostic: observation.diagnostic,
                })
            }
            Self::Name(name) => {
                let resolved = SessionRegistry::open_default()?.resolve_lifecycle(name)?;

                Ok(LocatedSession {
                    path: resolved.session,
                    phase: resolved.lifecycle,
                    diagnostic: resolved.diagnostic,
                })
            }
        };
    }
}
