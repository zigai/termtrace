//! Typed errors shared by the CLI and session runtime.

use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};

use miette::Diagnostic;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    client::{ActionResult, StableScreenCapture},
    control::{ActorBoundaries, InputSequenceFailureDetails},
    screen_frame::ScreenFrameError,
};

/// Bounded retry fact attached to typed public actor failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrySafety {
    /// The exact same request identifier and payload may be retried idempotently.
    SameRequestOnly,
    /// No mutation occurred; a new request may be submitted after reevaluation.
    NewRequest,
    /// The available evidence cannot establish a safe automatic retry.
    Unknown,
}

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
    #[error("invalid JSON document `{path}` at `{json_path}`")]
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
    /// A bounded rich-screen projection request was invalid or exceeded its limits.
    #[error(transparent)]
    #[diagnostic(code(termtrace::screen_frame))]
    ScreenFrame(ScreenFrameError),
    /// A public action atom violated the shared bounded input contract.
    #[error("invalid input atom at index {atom_index}: {reason}")]
    #[diagnostic(code(termtrace::invalid_atom))]
    InvalidAtom {
        /// Zero-based atom index.
        atom_index: usize,
        /// Bounded validation reason.
        reason: String,
    },

    /// A public duration violated an exact operation-specific bound.
    #[error("invalid duration `{field}`: {reason}")]
    #[diagnostic(code(termtrace::invalid_duration))]
    InvalidDuration {
        /// Stable option field name.
        field: &'static str,
        /// Bounded validation reason.
        reason: String,
    },

    /// An optimistic action guard rejected before any input was applied.
    #[error("presented state changed from `{expected_state_hash}` to `{current_state_hash}`")]
    #[diagnostic(code(termtrace::stale_state))]
    StaleState {
        /// Caller-supplied expected state hash.
        expected_state_hash: String,
        /// Actor-current presented state hash.
        current_state_hash: String,
        /// Exact rejection ordering point.
        boundaries: Box<ActorBoundaries>,
        /// No input was applied; retry requires reevaluating with a new request.
        retry_safety: RetrySafety,
    },

    /// Actor exclusivity rejected a new action before mutation.
    #[error("actor action is busy: {message}")]
    #[diagnostic(code(termtrace::action_busy))]
    ActionBusy {
        /// Bounded actor diagnostic.
        message: String,
    },

    /// An admitted input action retained a known applied prefix.
    #[error("partial input stopped after a bounded applied prefix")]
    #[diagnostic(code(termtrace::partial_input))]
    PartialInput {
        /// Complete bounded actor prefix accounting.
        details: InputSequenceFailureDetails,
        /// Exact same-request retry is idempotent but does not apply the suffix.
        retry_safety: RetrySafety,
        /// Whether any bytes of the failed atom were written.
        uncertain: bool,
    },

    /// An action applied every input atom but failed to project its terminating frame.
    #[error(
        "action `{request_id}` applied all input but could not project its terminating frame: {source}"
    )]
    #[diagnostic(code(termtrace::action_terminating_screen_frame))]
    ActionTerminatingFrame {
        /// Caller-retainable identifier required for an idempotent retry.
        request_id: Uuid,
        /// Exact failure ordering point after complete input application.
        boundaries: Box<ActorBoundaries>,
        /// Input was completely applied before projection failed.
        input_fully_applied: bool,
        /// Only the exact same request identifier and payload may be retried.
        retry_safety: RetrySafety,
        /// Typed terminating-frame projection failure.
        #[source]
        source: ScreenFrameError,
    },

    /// An action returned a non-success terminal quality under strict policy.
    #[error("action postcondition did not complete successfully")]
    #[diagnostic(code(termtrace::postcondition_incomplete))]
    PostconditionIncomplete {
        /// Complete exact action result, including its terminating frame.
        result: Box<ActionResult>,
    },

    /// A stable capture returned a non-stable terminal quality under strict policy.
    #[error("stable capture did not reach the requested stable interval")]
    #[diagnostic(code(termtrace::stable_capture_incomplete))]
    StableCaptureIncomplete {
        /// Complete exact stable capture, including its terminating frame.
        result: Box<StableScreenCapture>,
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
        /// Zero-based atom whose encoding or PTY delivery failed.
        failed_atom_index: u32,
        /// Caller-owned identifier of the failed atom.
        failed_operation_id: uuid::Uuid,
        /// Number of preceding atoms applied completely.
        atoms_applied: u32,
        /// Number of bytes written by all applied or partially applied atoms.
        bytes_written: u64,
        /// Number of bytes written for the failed atom; zero for an encoding failure.
        failed_atom_bytes_written: u64,
        /// First intent event in the applied or attempted prefix.
        first_intent_event_seq: u64,
        /// Last result event in the applied or attempted prefix.
        last_result_event_seq: u64,
        /// Underlying encoding or PTY delivery failure.
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

    /// A bounded runtime capability probe rejected an optional operation.
    #[error("required capability `{id}` is blocked ({code}): {detail}")]
    #[diagnostic(
        code(termtrace::capability_blocked),
        help("{remediation}; inspect with `termtrace tooling doctor --require {id} --explain`")
    )]
    CapabilityBlocked {
        /// Stable capability identifier.
        id: String,
        /// Stable probe diagnostic code.
        code: String,
        /// Human-readable probe result.
        detail: String,
        /// Focused remediation copied from the capability report.
        remediation: String,
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
