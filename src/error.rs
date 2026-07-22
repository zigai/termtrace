//! Typed errors shared by the CLI and session runtime.

use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};

use miette::Diagnostic;
use thiserror::Error;

/// Failures exposed by Termtrace's public interfaces.
#[derive(Debug, Error, Diagnostic)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("filesystem operation failed for `{path}`")]
    #[diagnostic(
        code(termtrace::filesystem),
        help("Check that the path exists and is accessible by the current user")
    )]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying operating-system error.
        #[source]
        source: IoError,
    },

    /// A persisted JSON document did not match its schema.
    #[error("invalid JSON document `{path}`")]
    #[diagnostic(
        code(termtrace::invalid_json),
        help("Preserve the original file for diagnosis; derived artifacts can be regenerated")
    )]
    InvalidJson {
        /// Invalid document path.
        path: PathBuf,
        /// Schema location reported by the streaming deserializer.
        json_path: String,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// A recording violates the event-log format or integrity chain.
    #[error("invalid event log `{path}` at line {line}: {reason}")]
    #[diagnostic(
        code(termtrace::invalid_event_log),
        help(
            "Preserve the original recording; valid events before the failure remain recoverable"
        )
    )]
    InvalidEventLog {
        /// Recording path.
        path: PathBuf,
        /// One-based line number.
        line: u64,
        /// Validation failure.
        reason: String,
    },

    /// The system clock cannot be represented by the event format.
    #[error("system clock is earlier than the Unix epoch")]
    #[diagnostic(code(termtrace::clock_before_epoch))]
    ClockBeforeEpoch,

    /// A session artifact directory could expose sensitive terminal data.
    #[error("insecure artifact directory `{path}` (owner uid {owner_uid}, mode {mode:#o})")]
    #[diagnostic(
        code(termtrace::insecure_artifact_directory),
        help("Use a directory owned by the current user with mode 0700")
    )]
    InsecureArtifactDirectory {
        /// Directory that failed validation.
        path: PathBuf,
        /// Filesystem owner user ID.
        owner_uid: u32,
        /// Permission bits.
        mode: u32,
    },

    /// A PTY or child-process operation failed.
    #[error("terminal operation `{operation}` failed: {reason}")]
    #[diagnostic(code(termtrace::terminal_operation))]
    TerminalOperation {
        /// Stable operation name.
        operation: &'static str,
        /// Error returned by the PTY implementation.
        reason: String,
    },

    /// The requested session directory already exists.
    #[error("session directory `{path}` already exists")]
    #[diagnostic(
        code(termtrace::session_exists),
        help("Choose a new directory; Termtrace never overwrites a recording")
    )]
    SessionExists {
        /// Existing path.
        path: PathBuf,
    },

    /// Terminal pixel geometry overflowed the PTY representation.
    #[error(
        "terminal geometry {cols}x{rows} at {cell_width_px}x{cell_height_px}px per cell is too large"
    )]
    #[diagnostic(code(termtrace::invalid_geometry))]
    InvalidGeometry {
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
        /// Cell width in pixels.
        cell_width_px: u16,
        /// Cell height in pixels.
        cell_height_px: u16,
    },

    /// Cleanup failed after a primary runtime failure.
    #[error("{primary}; cleanup also failed: {cleanup}")]
    #[diagnostic(code(termtrace::cleanup_failed))]
    CleanupFailed {
        /// Failure that caused the operation to stop.
        #[source]
        primary: Box<Error>,
        /// Independent failure encountered while restoring host state.
        cleanup: Box<Error>,
    },

    /// An event-log writer cannot safely continue after a partial I/O failure.
    #[error("event log writer for `{path}` is poisoned by an earlier I/O failure")]
    #[diagnostic(
        code(termtrace::event_log_poisoned),
        help("Stop the session and preserve the recording for prefix recovery")
    )]
    EventLogPoisoned {
        /// Event-log path.
        path: PathBuf,
    },

    /// A control request or response violated the versioned protocol.
    #[error("invalid control protocol message: {reason}")]
    #[diagnostic(code(termtrace::invalid_control_protocol))]
    InvalidControlProtocol {
        /// Protocol validation failure.
        reason: String,
    },

    /// A live session rejected a well-formed control request.
    #[error("live session rejected control request `{code}`: {message}")]
    #[diagnostic(code(termtrace::control_rejected))]
    ControlRejected {
        /// Stable machine-readable rejection code.
        code: String,
        /// Human-readable context.
        message: String,
    },

    /// A session has no reachable live control actor.
    #[error("live control is unavailable for session `{session}` ({lifecycle})")]
    #[diagnostic(
        code(termtrace::control_unavailable),
        help(
            "Run `termtrace status SESSION` to inspect lifecycle state; completed sessions support offline observation but no further mutation"
        )
    )]
    ControlUnavailable {
        /// Session directory whose actor could not be reached.
        session: PathBuf,
        /// Best-effort lifecycle classification from canonical files.
        lifecycle: String,
        /// Underlying socket error.
        #[source]
        source: IoError,
    },

    /// A dispatched control request did not return before its bounded deadline.
    #[error(
        "live control timed out during `{phase}` for session `{session}` after {timeout_ms} ms"
    )]
    #[diagnostic(
        code(termtrace::control_timeout),
        help(
            "For a mutation, retry the exact same request ID so the actor journal prevents duplicate application"
        )
    )]
    ControlTimeout {
        /// Session directory containing the control endpoint.
        session: PathBuf,
        /// Transport phase that exceeded its deadline.
        phase: &'static str,
        /// Configured bounded timeout.
        timeout_ms: u64,
        /// Whether any request bytes may already have reached the actor.
        request_dispatched: bool,
        /// Underlying timed-out I/O operation.
        #[source]
        source: IoError,
    },

    /// A control transport failed after a request may have reached the actor.
    #[error("live control transport failed during `{phase}` for session `{session}`")]
    #[diagnostic(
        code(termtrace::control_transport),
        help(
            "When request delivery may be uncertain, retry the exact same request ID and payload"
        )
    )]
    ControlTransport {
        /// Session directory containing the control endpoint.
        session: PathBuf,
        /// Transport phase that failed.
        phase: &'static str,
        /// Whether any request bytes may already have reached the actor.
        request_dispatched: bool,
        /// Underlying socket error.
        #[source]
        source: IoError,
    },

    /// A canonical input result retained a failed or partial PTY write.
    #[error("PTY input operation `{operation_id}` failed after {bytes_written} bytes: {reason}")]
    #[diagnostic(code(termtrace::input_write_failed))]
    InputWriteFailed {
        /// Canonical operation identifier.
        operation_id: uuid::Uuid,
        /// Bytes written before the failure.
        bytes_written: u64,
        /// Canonical input intent event.
        intent_event_seq: u64,
        /// Canonical failed input result event.
        result_event_seq: u64,
        /// Underlying terminal write error.
        reason: String,
    },

    /// A multi-atom input sequence stopped after retaining its exact applied prefix.
    #[error("input sequence `{sequence_id}` stopped at atom {failed_atom_index}: {reason}")]
    #[diagnostic(
        code(termtrace::input_sequence_incomplete),
        help(
            "Inspect the structured control details before deciding whether to submit the unattempted suffix with a new request ID"
        )
    )]
    InputSequenceIncomplete {
        /// Caller-owned sequence request identifier.
        sequence_id: uuid::Uuid,
        /// Zero-based atom that failed after its intent was recorded.
        failed_atom_index: u32,
        /// Caller-owned identifier of the failed atom.
        failed_operation_id: uuid::Uuid,
        /// Number of preceding atoms applied completely.
        atoms_applied: u32,
        /// Number of bytes written by all attempted atoms, including a partial failed atom.
        bytes_written: u64,
        /// Number of bytes written for the failed atom.
        failed_atom_bytes_written: u64,
        /// First intent event in the attempted prefix.
        first_intent_event_seq: u64,
        /// Result event for the failed atom.
        last_result_event_seq: u64,
        /// Underlying terminal write failure.
        reason: String,
    },

    /// The pinned libghostty-vt semantic engine rejected an operation.
    #[error("libghostty-vt operation `{operation}` failed with result {result}")]
    #[diagnostic(code(termtrace::libghostty_vt))]
    GhosttyVt {
        /// Stable adapter operation name.
        operation: &'static str,
        /// Upstream `GhosttyResult` numeric value.
        result: i32,
    },

    /// Artifact output already exists and is never replaced implicitly.
    #[error("artifact directory `{path}` already exists")]
    #[diagnostic(
        code(termtrace::derived_artifacts_exist),
        help("Move or remove the existing derived directory before indexing again")
    )]
    ArtifactExists {
        /// Existing derived artifact directory.
        path: PathBuf,
    },

    /// An immutable post-processing command was attempted before recording ended.
    #[error("session `{session}` is not complete")]
    #[diagnostic(
        code(termtrace::session_incomplete),
        help(
            "Stop the recording and wait for completion, or use `index --live-snapshot` for an explicitly incomplete prefix"
        )
    )]
    SessionIncomplete {
        /// Session directory without a durable completion manifest.
        session: PathBuf,
    },

    /// Canonical events cannot be deterministically replayed.
    #[error("invalid replay input: {reason}")]
    #[diagnostic(code(termtrace::invalid_replay))]
    InvalidReplay {
        /// Replay invariant that was violated.
        reason: String,
    },

    /// A required external runtime command failed.
    #[error("external command `{program}` failed: {reason}")]
    #[diagnostic(code(termtrace::external_command))]
    ExternalCommand {
        /// Executable or component name.
        program: String,
        /// Exit status or launch failure.
        reason: String,
    },

    /// Native capture failed but retained private diagnostic evidence.
    #[error("native capture failed; evidence preserved in `{evidence}`: {source}")]
    #[diagnostic(code(termtrace::capture_failed))]
    CaptureFailed {
        /// Owner-only failed-capture directory.
        evidence: PathBuf,
        /// Primary capture failure.
        #[source]
        source: Box<Error>,
    },
}

/// Result type used throughout Termtrace.
pub type Result<T> = StdResult<T, Error>;
