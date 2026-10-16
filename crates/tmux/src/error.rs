use crate::{artifact::TmuxRecordingLifecycle, lifecycle::LifecycleTrigger};
use std::{error::Error as StdError, io, path::PathBuf, result::Result as StdResult};

/// Failure of an aggregate tmux recording operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed for the named path.
    #[error("filesystem operation failed for `{path}`: {source}")]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O failure.
        source: io::Error,
    },
    /// A JSON document or record could not be encoded or decoded.
    #[error("invalid JSON in `{path}`: {source}")]
    Json {
        /// Document or record location.
        path: PathBuf,
        /// Underlying JSON failure.
        source: serde_json::Error,
    },
    /// The derived SQLite catalog failed.
    #[error("tmux catalog `{path}` failed: {source}")]
    Index {
        /// Catalog path.
        path: PathBuf,
        /// Underlying SQLite failure.
        source: rusqlite::Error,
    },
    /// Stored or observed tmux state violates the recording contract.
    #[error("{reason}")]
    Invalid {
        /// Stable explanation of the violated contract.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// A tmux command could not run or failed.
    #[error("{0}")]
    Tmux(String),
    /// Canonical session evidence could not be read.
    #[error(transparent)]
    Evidence(#[from] termtrace_evidence::Error),
    /// Session completion could not be observed.
    #[error(transparent)]
    Control(#[from] termtrace_control::Error),
    /// The reachable tmux server is not the one this recording bound.
    #[error("tmux server identity changed for aggregate {name}: {detail}")]
    ServerIdentityChanged {
        /// Aggregate recording name.
        name: String,
        /// Expected and observed identities.
        detail: String,
    },
    /// A lifecycle trigger is not legal in the current lifecycle.
    #[error("tmux recording cannot apply {trigger:?} while {from:?}")]
    IllegalTransition {
        /// Lifecycle before the trigger.
        from: TmuxRecordingLifecycle,
        /// Rejected trigger.
        trigger: LifecycleTrigger,
    },
    /// A completed recording's history is sealed.
    #[error("tmux recording `{0}` is completed; its history is sealed")]
    Sealed(String),
    /// The recording name is already taken.
    #[error("tmux recording `{0}` already exists")]
    Exists(String),
    /// No recording has this name.
    #[error("tmux recording `{0}` was not found")]
    NotFound(String),
    /// A track operation supplied by the caller failed.
    #[error("{operation} failed: {message}")]
    Track {
        /// Failed operation.
        operation: &'static str,
        /// Caller diagnostic.
        message: String,
    },
}

impl Error {
    /// Whether tmux failed because no server listens on the selected socket.
    pub fn is_server_unavailable(&self) -> bool {
        return matches!(
            self,
            Self::Tmux(message)
                if message.contains("no server running") || message.contains("error connecting to")
        );
    }

    pub(crate) fn filesystem(path: impl Into<PathBuf>, source: io::Error) -> Self {
        return Self::Filesystem {
            path: path.into(),
            source,
        };
    }

    pub(crate) fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        return Self::Json {
            path: path.into(),
            source,
        };
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        return Self::Invalid {
            reason: reason.into(),
            source: None,
        };
    }

    pub(crate) fn invalid_with_source(
        reason: impl Into<String>,
        source: impl Into<Box<dyn StdError + Send + Sync>>,
    ) -> Self {
        return Self::Invalid {
            reason: reason.into(),
            source: Some(source.into()),
        };
    }
}

/// Result of an aggregate tmux recording operation.
pub type Result<T, E = Error> = StdResult<T, E>;
