//! Failures produced by semantic and native media operations.

use std::{error::Error as StdError, fmt, io, path::PathBuf, result::Result as StdResult};
use termtrace_engine::Error as EngineError;
use termtrace_evidence::Error as EvidenceError;
use termtrace_render::Error as RenderError;
use termtrace_terminal::Error as TerminalError;

/// Failure while planning, capturing, rendering, playing, or publishing media.
#[derive(Debug)]
pub enum Error {
    /// A filesystem path could not be read, written, or published.
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying operating-system error.
        source: io::Error,
    },
    /// A media JSON document could not be decoded.
    InvalidJson {
        /// Path of the invalid JSON document.
        path: PathBuf,
        /// JSONPath identifying the invalid value when available.
        json_path: String,
        /// Underlying JSON decoding error.
        source: serde_json::Error,
    },
    /// Recorded evidence or requested media parameters violate the media contract.
    InvalidReplay {
        /// Actionable reason the input was rejected.
        reason: String,
    },
    /// A media operation requires a completed recording.
    SessionIncomplete {
        /// Incomplete session directory.
        session: PathBuf,
    },
    /// Atomic publication refused to replace an existing artifact.
    ArtifactExists {
        /// Existing artifact path.
        path: PathBuf,
    },
    /// Ghostty terminal setup or interaction failed.
    TerminalOperation {
        /// Terminal operation that failed.
        operation: &'static str,
        /// Actionable failure reason.
        reason: String,
    },
    /// The system clock cannot be represented relative to the Unix epoch.
    ClockBeforeEpoch,
    /// A required external media command failed.
    ExternalCommand {
        /// Command or command-stage description.
        operation: String,
        /// Actionable process failure reason.
        reason: String,
    },
    /// Media production was blocked by a required capability probe.
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
    CaptureFailed {
        /// Directory containing retained diagnostic artifacts.
        evidence: PathBuf,
        /// Underlying capture failure.
        source: Box<Error>,
    },
    /// A primary media failure was followed by a cleanup failure.
    CleanupFailed {
        /// Original media failure.
        primary: Box<Error>,
        /// Failure encountered while restoring or removing resources.
        cleanup: Box<Error>,
    },
    /// Evidence reading, validation, or publication failed.
    Evidence(EvidenceError),
    /// Replay or semantic analysis failed.
    Engine(EngineError),
    /// Terminal emulation failed.
    Terminal(TerminalError),
    /// Deterministic semantic rendering failed.
    Render(RenderError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filesystem { path, source } => {
                return write!(
                    f,
                    "filesystem operation failed for `{}`: {source}",
                    path.display()
                );
            }
            Self::InvalidJson {
                path,
                json_path,
                source,
            } => {
                return write!(
                    f,
                    "invalid JSON in `{}` at `{json_path}`: {source}",
                    path.display()
                );
            }
            Self::InvalidReplay { reason } => {
                return write!(f, "invalid media input: {reason}");
            }
            Self::SessionIncomplete { session } => {
                return write!(f, "session `{}` is incomplete", session.display());
            }
            Self::ArtifactExists { path } => {
                return write!(f, "media artifact already exists at `{}`", path.display());
            }
            Self::TerminalOperation { operation, reason } => {
                return write!(f, "terminal operation `{operation}` failed: {reason}");
            }
            Self::ClockBeforeEpoch => {
                return f.write_str("system clock is before the Unix epoch");
            }
            Self::ExternalCommand { operation, reason } => {
                return write!(f, "{operation} failed: {reason}");
            }
            Self::CapabilityBlocked {
                id, code, detail, ..
            } => {
                return write!(
                    f,
                    "required capability `{id}` is blocked ({code}): {detail}"
                );
            }
            Self::CaptureFailed { evidence, source } => {
                return write!(
                    f,
                    "native capture failed; diagnostic evidence retained at `{}`: {source}",
                    evidence.display()
                );
            }
            Self::CleanupFailed { primary, cleanup } => {
                return write!(f, "{primary}; cleanup also failed: {cleanup}");
            }
            Self::Evidence(source) => return write!(f, "{source}"),
            Self::Engine(source) => return write!(f, "{source}"),
            Self::Terminal(source) => return write!(f, "{source}"),
            Self::Render(source) => return write!(f, "{source}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Filesystem { source, .. } => return Some(source),
            Self::InvalidJson { source, .. } => return Some(source),
            Self::CaptureFailed { source, .. } => return Some(source),
            Self::CleanupFailed { primary, .. } => return Some(primary),
            Self::Evidence(source) => return Some(source),
            Self::Engine(source) => return Some(source),
            Self::Terminal(source) => return Some(source),
            Self::Render(source) => return Some(source),
            _ => return None,
        }
    }
}

impl From<EvidenceError> for Error {
    fn from(source: EvidenceError) -> Self {
        return Self::Evidence(source);
    }
}

impl From<EngineError> for Error {
    fn from(source: EngineError) -> Self {
        return Self::Engine(source);
    }
}

impl From<TerminalError> for Error {
    fn from(source: TerminalError) -> Self {
        return Self::Terminal(source);
    }
}

impl From<RenderError> for Error {
    fn from(source: RenderError) -> Self {
        return Self::Render(source);
    }
}

/// Result returned by media operations.
pub type Result<T, E = Error> = StdResult<T, E>;
