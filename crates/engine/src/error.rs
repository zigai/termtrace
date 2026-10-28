//! Failures produced while coordinating Termtrace engine operations.

use std::{
    error::Error as StdError, fmt, io, path::PathBuf, result::Result as StdResult, str::Utf8Error,
    time::SystemTimeError,
};
use termtrace_model::{
    error::{ModelValidationError, ThreadPanicError},
    execution::visual_profile::VisualProfileFailure,
    terminal::{screen::ScreenFrameError, state::terminal_geometry_limits},
};
use uuid::Uuid;

/// Failure from recording, replay, observation, analysis, or registry coordination.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed for the named path.
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O failure.
        source: io::Error,
    },
    /// JSON could not be decoded at the reported structural location.
    InvalidJson {
        /// File containing the invalid JSON.
        path: PathBuf,
        /// JSON path at which decoding failed.
        json_path: String,
        /// Underlying JSON decoding failure.
        source: serde_json::Error,
    },
    /// A canonical event-log record is malformed.
    InvalidEventLog {
        /// Event-log path.
        path: PathBuf,
        /// One-based line containing the invalid record.
        line: u64,
        /// Validation failure description.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// An event-log writer cannot continue after an earlier write failure.
    EventLogPoisoned {
        /// Path of the poisoned event log.
        path: PathBuf,
    },
    /// Recording cannot create a session because its directory already exists.
    SessionExists {
        /// Conflicting session directory.
        path: PathBuf,
    },
    /// An operation requires a completion receipt that the session lacks.
    SessionIncomplete {
        /// Incomplete session directory.
        session: PathBuf,
    },
    /// A derived artifact cannot be published because its directory exists.
    ArtifactExists {
        /// Conflicting artifact directory.
        path: PathBuf,
    },
    /// A no-replacement export destination already exists.
    DestinationExists {
        /// Conflicting destination path.
        path: PathBuf,
    },
    /// An artifact directory fails owner or permission checks.
    InsecureArtifactDirectory {
        /// Insecure directory path.
        path: PathBuf,
        /// Observed owner user ID.
        owner_uid: u32,
        /// Observed Unix permission mode.
        mode: u32,
    },
    /// Canonical evidence violates a required invariant.
    InvalidEvidence {
        /// Evidence validation failure description.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// Requested terminal dimensions exceed supported geometry.
    InvalidGeometry {
        /// Terminal column count.
        cols: u16,
        /// Terminal row count.
        rows: u16,
        /// Cell width in pixels.
        cell_width_px: u16,
        /// Cell height in pixels.
        cell_height_px: u16,
    },
    /// A terminal integration operation failed.
    TerminalOperation {
        /// Name of the failed operation.
        operation: &'static str,
        /// Failure description.
        reason: String,
    },
    /// A supervised worker thread panicked before it could be joined cleanly.
    ThreadPanicked {
        /// Name of the join operation.
        operation: &'static str,
        /// Captured panic.
        source: ThreadPanicError,
    },
    /// Libghostty-vt returned text that is not valid UTF-8.
    InvalidTerminalText {
        /// Name of the failed libghostty-vt operation.
        operation: &'static str,
        /// Underlying decoding failure.
        source: Utf8Error,
    },
    /// Libghostty-vt returned a non-success result.
    GhosttyVt {
        /// Name of the failed libghostty-vt operation.
        operation: &'static str,
        /// Numeric result returned by libghostty-vt.
        result: i32,
    },
    /// A virtual-terminal byte stream is malformed or unsupported.
    InvalidVtStream {
        /// Stream validation failure description.
        reason: String,
    },
    /// A requested screen-frame projection is invalid.
    ScreenFrame(ScreenFrameError),
    /// A control-socket message violates the protocol.
    InvalidControlProtocol {
        /// Protocol validation failure description.
        reason: String,
    },
    /// A live recording rejected a valid control request.
    ControlRejected {
        /// Stable machine-readable rejection code.
        code: String,
        /// Human-readable rejection description.
        message: String,
    },
    /// A PTY input write stopped after a durable intent was recorded.
    InputWriteFailed {
        /// Input operation identifier.
        operation_id: Uuid,
        /// Number of bytes written before failure.
        bytes_written: u64,
        /// Event sequence of the durable input intent.
        intent_event_seq: u64,
        /// Event sequence recording the failed result.
        result_event_seq: u64,
        /// Write failure description.
        reason: String,
    },
    /// A paced input sequence stopped before every atom completed.
    InputSequenceIncomplete {
        /// Input-sequence identifier.
        sequence_id: Uuid,
        /// Zero-based index of the failed atom.
        failed_atom_index: u32,
        /// Operation identifier of the failed atom.
        failed_operation_id: Uuid,
        /// Number of atoms completed before failure.
        atoms_applied: u32,
        /// Total bytes written by completed and partial atoms.
        bytes_written: u64,
        /// Bytes written for the failed atom.
        failed_atom_bytes_written: u64,
        /// Event sequence of the first durable input intent.
        first_intent_event_seq: u64,
        /// Event sequence of the final recorded result.
        last_result_event_seq: u64,
        /// Sequence failure description.
        reason: String,
    },
    /// Cleanup failed after an earlier primary operation failure.
    CleanupFailed {
        /// Original operation failure.
        primary: Box<Error>,
        /// Subsequent cleanup failure.
        cleanup: Box<Error>,
    },
    /// Replay input or derived replay state violates an invariant.
    InvalidReplay {
        /// Replay validation failure description.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// A checkpoint VT seed cannot reconstruct its committed semantic state.
    CheckpointReconstruction {
        /// Semantic fields that differ after restoring the checkpoint seed.
        mismatched_fields: Vec<&'static str>,
    },
    /// A required runtime capability is unavailable.
    CapabilityBlocked {
        /// Capability identifier.
        id: String,
        /// Stable machine-readable block code.
        code: String,
        /// Human-readable block description.
        detail: String,
        /// Suggested action for enabling the capability.
        remediation: String,
    },
    /// The system clock cannot be represented as time since the Unix epoch.
    ClockBeforeEpoch {
        /// Underlying clock comparison failure.
        source: SystemTimeError,
    },
    /// An external command could not be executed successfully.
    ExternalCommand {
        /// Purpose of the external command.
        operation: &'static str,
        /// Command failure description.
        reason: String,
    },
    /// A stable model value failed its own validation contract.
    ModelValidation(ModelValidationError),
    /// A visual profile could not be validated or resolved; no substitute was used.
    VisualProfile {
        /// Stable failure category.
        failure: VisualProfileFailure,
        /// Failure description.
        reason: String,
    },
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
            Self::InvalidEventLog {
                path, line, reason, ..
            } => {
                return write!(
                    f,
                    "invalid event log `{}` at line {line}: {reason}",
                    path.display()
                );
            }
            Self::EventLogPoisoned { path } => {
                return write!(f, "event log writer for `{}` is poisoned", path.display());
            }
            Self::SessionExists { path } => {
                return write!(f, "session directory `{}` already exists", path.display());
            }
            Self::SessionIncomplete { session } => {
                return write!(f, "session `{}` is incomplete", session.display());
            }
            Self::ArtifactExists { path } => {
                return write!(f, "artifact directory `{}` already exists", path.display());
            }
            Self::DestinationExists { path } => {
                return write!(f, "destination already exists at `{}`", path.display());
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
            Self::InvalidEvidence { reason, .. } => return write!(f, "invalid evidence: {reason}"),
            Self::InvalidGeometry {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => {
                return write!(
                    f,
                    "terminal geometry {cols}x{rows} at {cell_width_px}x{cell_height_px}px per cell is not supported; the grid must fit {} and its pixel size must fit 65535x65535",
                    terminal_geometry_limits()
                );
            }
            Self::TerminalOperation { operation, reason } => {
                return write!(f, "terminal operation `{operation}` failed: {reason}");
            }
            Self::GhosttyVt { operation, result } => {
                return write!(
                    f,
                    "libghostty-vt operation `{operation}` failed with result {result}"
                );
            }
            Self::InvalidVtStream { reason } => return write!(f, "invalid VT stream: {reason}"),
            Self::ScreenFrame(error) => return write!(f, "{error}"),
            Self::InvalidControlProtocol { reason } => {
                return write!(f, "invalid control protocol message: {reason}");
            }
            Self::ControlRejected { code, message } => {
                return write!(
                    f,
                    "live session rejected control request `{code}`: {message}"
                );
            }
            Self::InputWriteFailed {
                operation_id,
                bytes_written,
                reason,
                ..
            } => {
                return write!(
                    f,
                    "PTY input operation `{operation_id}` failed after {bytes_written} bytes: {reason}"
                );
            }
            Self::InputSequenceIncomplete {
                sequence_id,
                failed_atom_index,
                reason,
                ..
            } => {
                return write!(
                    f,
                    "input sequence `{sequence_id}` stopped at atom {failed_atom_index}: {reason}"
                );
            }
            Self::CleanupFailed { primary, cleanup } => {
                return write!(f, "{primary}; cleanup also failed: {cleanup}");
            }
            Self::InvalidReplay { reason, .. } => {
                return write!(f, "invalid replay input: {reason}");
            }
            Self::CheckpointReconstruction { mismatched_fields } => {
                return write!(
                    f,
                    "replay checkpoint VT seed does not reconstruct semantic fields: {}",
                    mismatched_fields.join(", ")
                );
            }
            Self::CapabilityBlocked {
                id, code, detail, ..
            } => {
                return write!(
                    f,
                    "required capability `{id}` is blocked ({code}): {detail}"
                );
            }
            Self::ClockBeforeEpoch { .. } => {
                return f.write_str("system clock is before the Unix epoch");
            }
            Self::ThreadPanicked { operation, source } => {
                return write!(f, "terminal operation `{operation}` failed: {source}");
            }
            Self::InvalidTerminalText { operation, source } => {
                return write!(
                    f,
                    "libghostty-vt {operation} returned invalid UTF-8: {source}"
                );
            }
            Self::ExternalCommand { operation, reason } => {
                return write!(f, "{operation} failed: {reason}");
            }
            Self::ModelValidation(source) => return write!(f, "model validation failed: {source}"),
            Self::VisualProfile { failure, reason } => {
                return write!(f, "visual profile {}: {reason}", failure.code());
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Filesystem { source, .. } => return Some(source),
            Self::InvalidJson { source, .. } => return Some(source),
            Self::ScreenFrame(source) => return Some(source),
            Self::CleanupFailed { primary, .. } => return Some(primary),
            Self::ModelValidation(source) => return Some(source),
            Self::InvalidEvidence {
                source: Some(source),
                ..
            }
            | Self::InvalidEventLog {
                source: Some(source),
                ..
            }
            | Self::InvalidReplay {
                source: Some(source),
                ..
            } => return Some(source.as_ref()),
            Self::ThreadPanicked { source, .. } => return Some(source),
            Self::InvalidTerminalText { source, .. } => return Some(source),
            Self::ClockBeforeEpoch { source } => return Some(source),
            _ => return None,
        }
    }
}

impl From<termtrace_evidence::Error> for Error {
    fn from(source: termtrace_evidence::Error) -> Self {
        match source {
            termtrace_evidence::Error::Filesystem { path, source } => {
                return Self::Filesystem { path, source };
            }
            termtrace_evidence::Error::InvalidJson {
                path,
                json_path,
                source,
            } => {
                return Self::InvalidJson {
                    path,
                    json_path,
                    source,
                };
            }
            termtrace_evidence::Error::InvalidEventLog {
                path,
                line,
                reason,
                source,
            } => {
                return Self::InvalidEventLog {
                    path,
                    line,
                    reason,
                    source,
                };
            }
            termtrace_evidence::Error::InsecureArtifactDirectory {
                path,
                owner_uid,
                mode,
            } => {
                return Self::InsecureArtifactDirectory {
                    path,
                    owner_uid,
                    mode,
                };
            }
            termtrace_evidence::Error::InvalidEvidence { reason, source } => {
                return Self::InvalidEvidence { reason, source };
            }
            termtrace_evidence::Error::SessionIncomplete { session } => {
                return Self::SessionIncomplete { session };
            }
            termtrace_evidence::Error::DestinationExists { path } => {
                return Self::DestinationExists { path };
            }
            termtrace_evidence::Error::EventLogPoisoned { path } => {
                return Self::EventLogPoisoned { path };
            }
            termtrace_evidence::Error::ExternalCommand { operation, reason } => {
                return Self::ExternalCommand { operation, reason };
            }
            termtrace_evidence::Error::ClockBeforeEpoch { source } => {
                return Self::ClockBeforeEpoch { source };
            }
            termtrace_evidence::Error::ModelValidation(source) => {
                return Self::ModelValidation(source);
            }
        }
    }
}

impl From<termtrace_terminal::Error> for Error {
    fn from(source: termtrace_terminal::Error) -> Self {
        match source {
            termtrace_terminal::Error::GhosttyVt { operation, result } => {
                return Self::GhosttyVt { operation, result };
            }
            termtrace_terminal::Error::TerminalOperation { operation, reason } => {
                return Self::TerminalOperation { operation, reason };
            }
            termtrace_terminal::Error::InvalidVtStream { reason } => {
                return Self::InvalidVtStream { reason };
            }
            termtrace_terminal::Error::InvalidUtf8 { operation, source } => {
                return Self::InvalidTerminalText { operation, source };
            }
        }
    }
}

impl From<ModelValidationError> for Error {
    fn from(source: ModelValidationError) -> Self {
        return Self::ModelValidation(source);
    }
}

/// Result returned by Termtrace engine operations.
pub type Result<T, E = Error> = StdResult<T, E>;
