//! Failures produced by semantic and native media operations.

use std::{
    error::Error as StdError, io, path::PathBuf, result::Result as StdResult, time::SystemTimeError,
};
use termtrace_engine::Error as EngineError;
use termtrace_evidence::Error as EvidenceError;
use termtrace_model::{artifact::original::OriginalCaptureFailure, error::ThreadPanicError};
use termtrace_render::Error as RenderError;
use termtrace_terminal::Error as TerminalError;

/// Failure while planning, capturing, rendering, playing, or publishing media.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem path could not be read, written, or published.
    #[error("filesystem operation failed for `{}`: {source}", path.display())]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying operating-system error.
        source: io::Error,
    },
    /// A media JSON document could not be decoded.
    #[error("invalid JSON in `{}` at `{json_path}`: {source}", path.display())]
    InvalidJson {
        /// Path of the invalid JSON document.
        path: PathBuf,
        /// JSONPath identifying the invalid value when available.
        json_path: String,
        /// Underlying JSON decoding error.
        source: serde_json::Error,
    },
    /// Recorded evidence or requested media parameters violate the media contract.
    #[error("invalid media input: {reason}")]
    InvalidReplay {
        /// Actionable reason the input was rejected.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// A media operation requires a completed recording.
    #[error("session `{}` is incomplete", session.display())]
    SessionIncomplete {
        /// Incomplete session directory.
        session: PathBuf,
    },
    /// Atomic publication refused to replace an existing artifact.
    #[error("media artifact already exists at `{}`", path.display())]
    ArtifactExists {
        /// Existing artifact path.
        path: PathBuf,
    },
    /// Ghostty terminal setup or interaction failed.
    #[error("terminal operation `{operation}` failed: {reason}")]
    TerminalOperation {
        /// Terminal operation that failed.
        operation: &'static str,
        /// Actionable failure reason.
        reason: String,
    },
    /// A media worker thread panicked before it could be joined cleanly.
    #[error("terminal operation `{operation}` failed: {source}")]
    ThreadPanicked {
        /// Join operation that failed.
        operation: &'static str,
        /// Captured panic.
        source: ThreadPanicError,
    },
    /// The system clock cannot be represented relative to the Unix epoch.
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch {
        /// Underlying clock comparison failure.
        source: SystemTimeError,
    },
    /// A required external media command failed.
    #[error("{operation} failed: {reason}")]
    ExternalCommand {
        /// Command or command-stage description.
        operation: String,
        /// Actionable process failure reason.
        reason: String,
    },
    /// Media production was blocked by a required capability probe.
    #[error("required capability `{id}` is blocked ({code}): {detail}")]
    CapabilityBlocked {
        /// Stable capability identifier.
        id: String,
        /// Stable blocked-status code.
        code: String,
        /// Human-readable probe result.
        detail: String,
        /// Action required to make the capability available.
        remediation: String,
    },
    /// Native capture failed and retained diagnostic evidence for inspection.
    #[error("native capture failed; diagnostic evidence retained at `{}`: {source}", evidence.display())]
    CaptureFailed {
        /// Directory containing retained diagnostic artifacts.
        evidence: PathBuf,
        /// Underlying capture failure.
        source: Box<Error>,
    },
    /// An original-execution capture failed at a classified stage.
    #[error("original capture failed ({}): {source}", code.code())]
    OriginalCapture {
        /// Stable failure code.
        code: OriginalCaptureFailure,
        /// Underlying failure.
        source: Box<Error>,
    },
    /// A primary media failure was followed by a cleanup failure.
    #[error("{primary}; cleanup also failed: {cleanup}")]
    CleanupFailed {
        /// Original media failure.
        #[source]
        primary: Box<Error>,
        /// Failure encountered while restoring or removing resources.
        cleanup: Box<Error>,
    },
    /// Session phase observation failed.
    #[error("{0}")]
    Control(#[from] termtrace_control::Error),
    /// Evidence reading, validation, or publication failed.
    #[error("{0}")]
    Evidence(#[from] EvidenceError),
    /// Replay or semantic analysis failed.
    #[error("{0}")]
    Engine(#[from] EngineError),
    /// Terminal emulation failed.
    #[error("{0}")]
    Terminal(#[from] TerminalError),
    /// Deterministic semantic rendering failed.
    #[error("{0}")]
    Render(#[from] RenderError),
}

/// Result returned by media operations.
pub type Result<T, E = Error> = StdResult<T, E>;

pub(crate) fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}
