//! Typed errors shared by the CLI and session runtime.

use std::path::PathBuf;

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
        source: std::io::Error,
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
pub type Result<T> = std::result::Result<T, Error>;
