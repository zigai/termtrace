use std::{error::Error as StdError, fmt, io, path::PathBuf, result::Result as StdResult};
use termtrace_model::error::ModelValidationError;

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompletionIntegrityKind {
    CanonicalContent,
    SessionManifestHash,
    EventHeaderHash,
    TerminalExitMissing,
    PendingMutationIntents,
    EventCount,
    OutputCount,
    FinalEventHash,
    ExitOutcome,
    DurabilityRecord,
}

// Classification stays private; public validation retains its existing Error contract.
#[derive(Debug)]
pub(crate) struct CompletionIntegrityError {
    pub(crate) kind: CompletionIntegrityKind,
    pub(crate) error: Error,
}

impl CompletionIntegrityError {
    pub(crate) fn new(kind: CompletionIntegrityKind, reason: impl Into<String>) -> Self {
        return Self {
            kind,
            error: Error::invalid(reason),
        };
    }
}

impl From<Error> for CompletionIntegrityError {
    fn from(error: Error) -> Self {
        return Self {
            kind: CompletionIntegrityKind::CanonicalContent,
            error,
        };
    }
}

/// Failure while storing, reading, or verifying evidence.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed for the named path.
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O failure.
        source: io::Error,
    },
    /// Structured evidence contains invalid JSON.
    InvalidJson {
        /// File containing the invalid value.
        path: PathBuf,
        /// JSON path nearest the invalid value.
        json_path: String,
        /// Underlying JSON failure.
        source: serde_json::Error,
    },
    /// A canonical event log violates its format or chain contract.
    InvalidEventLog {
        /// Event-log path.
        path: PathBuf,
        /// One-based line containing the violation.
        line: u64,
        /// Stable explanation of the violated contract.
        reason: String,
    },
    /// An artifact directory does not have the required private ownership or mode.
    InsecureArtifactDirectory {
        /// Rejected directory.
        path: PathBuf,
        /// Observed owner user identifier.
        owner_uid: u32,
        /// Observed Unix permission mode.
        mode: u32,
    },
    /// Evidence violates a semantic integrity contract.
    InvalidEvidence {
        /// Stable explanation of the violation.
        reason: String,
    },
    /// A session has no valid durable completion record.
    SessionIncomplete {
        /// Incomplete session directory.
        session: PathBuf,
    },
    /// A create-only destination already exists.
    DestinationExists {
        /// Existing destination.
        path: PathBuf,
    },
    /// An event-log writer cannot continue after a persistence failure.
    EventLogPoisoned {
        /// Affected event-log path.
        path: PathBuf,
    },
    /// An external evidence operation failed.
    ExternalCommand {
        /// Stable operation label.
        operation: &'static str,
        /// Failure detail returned by the external operation.
        reason: String,
    },
    /// The system clock cannot be represented relative to the Unix epoch.
    ClockBeforeEpoch,
    /// A shared model value failed validation.
    ModelValidation(ModelValidationError),
}

impl Error {
    pub(crate) fn filesystem(path: impl Into<PathBuf>, source: io::Error) -> Self {
        return Self::Filesystem {
            path: path.into(),
            source,
        };
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        return Self::InvalidEvidence {
            reason: reason.into(),
        };
    }
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
            Self::InvalidEventLog { path, line, reason } => {
                return write!(
                    f,
                    "invalid event log `{}` at line {line}: {reason}",
                    path.display()
                );
            }
            Self::InsecureArtifactDirectory {
                path,
                owner_uid,
                mode,
            } => {
                return write!(
                    f,
                    "artifact directory `{}` must be owned by the effective user with mode 0700 (owner={owner_uid}, mode={mode:o})",
                    path.display()
                );
            }
            Self::InvalidEvidence { reason } => {
                return write!(f, "invalid evidence: {reason}");
            }
            Self::SessionIncomplete { session } => {
                return write!(f, "session `{}` is incomplete", session.display());
            }
            Self::DestinationExists { path } => {
                return write!(f, "destination already exists at `{}`", path.display());
            }
            Self::EventLogPoisoned { path } => {
                return write!(
                    f,
                    "event log writer for `{}` is poisoned after a persistence failure",
                    path.display()
                );
            }
            Self::ExternalCommand { operation, reason } => {
                return write!(f, "{operation} failed: {reason}");
            }
            Self::ClockBeforeEpoch => {
                return f.write_str("system clock is before the Unix epoch");
            }
            Self::ModelValidation(source) => {
                return write!(f, "model validation failed: {source}");
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Filesystem { source, .. } => return Some(source),
            Self::InvalidJson { source, .. } => return Some(source),
            Self::ModelValidation(source) => return Some(source),
            _ => return None,
        }
    }
}

impl From<ModelValidationError> for Error {
    fn from(source: ModelValidationError) -> Self {
        return Self::ModelValidation(source);
    }
}

/// Result of an evidence storage or verification operation.
pub type Result<T, E = Error> = StdResult<T, E>;
