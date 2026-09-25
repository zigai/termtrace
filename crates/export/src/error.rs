use std::{io, path::PathBuf, result::Result as StdResult};
use termtrace_engine::Error as EngineError;
use termtrace_evidence::Error as EvidenceError;
use termtrace_render::Error as RenderError;

/// Failure while planning, publishing, or verifying an evidence export.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Evidence collection, validation, or publication failed.
    #[error(transparent)]
    Evidence(#[from] EvidenceError),
    /// Replay or semantic analysis failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// Deterministic semantic rendering failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// Aggregate tmux history is unreadable or invalid.
    #[error(transparent)]
    Tmux(#[from] termtrace_tmux::Error),
    /// A filesystem path could not be read, written, or published.
    #[error("filesystem operation failed for `{path}`: {source}")]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying operating-system error.
        source: io::Error,
    },
    /// An export JSON document could not be decoded.
    #[error("invalid JSON in `{path}`: {source}")]
    InvalidJson {
        /// Path of the invalid JSON document.
        path: PathBuf,
        /// Underlying JSON decoding error.
        source: serde_json::Error,
    },
    /// Export content or policy violates the evidence export contract.
    #[error("invalid evidence export: {reason}")]
    InvalidExport {
        /// Actionable reason the export was rejected.
        reason: String,
    },
}

impl Error {
    pub(super) fn filesystem(path: impl Into<PathBuf>, source: io::Error) -> Self {
        return Self::Filesystem {
            path: path.into(),
            source,
        };
    }

    pub(super) fn invalid(reason: impl Into<String>) -> Self {
        return Self::InvalidExport {
            reason: reason.into(),
        };
    }

    pub(super) fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        return Self::InvalidJson {
            path: path.into(),
            source,
        };
    }
}

/// Result returned by evidence export operations.
pub type Result<T, E = Error> = StdResult<T, E>;
