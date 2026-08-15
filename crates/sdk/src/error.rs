//! Coherent errors exposed by the blocking SDK boundary.

use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};

use thiserror::Error;
use uuid::Uuid;

use termtrace_model::{
    protocol::control::InputSequenceFailureDetails,
    terminal::{observation::ActorBoundaries, screen::ScreenFrameError},
};

use crate::{actions::ActionResult, client::StableScreenCapture};

/// Bounded retry fact attached to typed public actor failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrySafety {
    /// Retry only the exact request identifier and payload.
    SameRequestOnly,
    /// No mutation occurred; reevaluate and submit a new request.
    NewRequest,
    /// Evidence cannot establish a safe automatic retry.
    Unknown,
}

/// Failures translated at the SDK boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// Canonical evidence access or verification failed.
    #[error(transparent)]
    Evidence(#[from] termtrace_evidence::Error),
    /// Replay, observation, analysis, or recording coordination failed.
    #[error(transparent)]
    Engine(#[from] termtrace_engine::Error),
    /// Evidence export failed.
    #[error(transparent)]
    Export(#[from] termtrace_export::Error),
    /// Media production failed.
    #[error(transparent)]
    Media(#[from] termtrace_media::Error),
    /// A filesystem operation failed.
    #[error("filesystem operation failed for `{path}`")]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        #[source]
        /// Underlying I/O failure.
        source: IoError,
    },
    /// A JSON protocol or manifest value was invalid.
    #[error("invalid JSON document `{path}` at `{json_path}`")]
    InvalidJson {
        /// Path of the invalid document.
        path: PathBuf,
        /// JSON path at which decoding failed.
        json_path: String,
        #[source]
        /// Underlying JSON decoding failure.
        source: serde_json::Error,
    },
    /// A control message violated framing or correlation rules.
    #[error("invalid control protocol message: {reason}")]
    InvalidControlProtocol {
        /// Bounded protocol validation failure.
        reason: String,
    },
    /// A live actor was unavailable.
    #[error("live control is unavailable for session `{session}` ({lifecycle})")]
    ControlUnavailable {
        /// Canonical session directory.
        session: PathBuf,
        /// Observed session lifecycle.
        lifecycle: String,
        #[source]
        /// Connection failure reported by the operating system.
        source: IoError,
    },
    /// A dispatched request exceeded its deadline.
    #[error(
        "live control timed out during `{phase}` for session `{session}` after {timeout_ms} ms"
    )]
    ControlTimeout {
        /// Canonical session directory.
        session: PathBuf,
        /// Transport phase that exceeded its deadline.
        phase: &'static str,
        /// Requested deadline in milliseconds.
        timeout_ms: u64,
        /// Whether the request may have reached the actor.
        request_dispatched: bool,
        #[source]
        /// Underlying timeout or transport failure.
        source: IoError,
    },
    /// Control transport failed with possibly uncertain delivery.
    #[error("live control transport failed during `{phase}` for session `{session}`")]
    ControlTransport {
        /// Canonical session directory.
        session: PathBuf,
        /// Transport phase that failed.
        phase: &'static str,
        /// Whether the request may have reached the actor.
        request_dispatched: bool,
        #[source]
        /// Underlying transport failure.
        source: IoError,
    },
    /// The actor rejected a well-formed request.
    #[error("live session rejected control request `{code}`: {message}")]
    ControlRejected {
        /// Stable actor rejection code.
        code: String,
        /// Safe actor rejection detail.
        message: String,
    },
    /// A rich-screen request was invalid or exceeded a bound.
    #[error(transparent)]
    ScreenFrame(ScreenFrameError),
    /// A public action atom violated the input contract.
    #[error("invalid input atom at index {atom_index}: {reason}")]
    InvalidAtom {
        /// Zero-based index of the invalid input atom.
        atom_index: usize,
        /// Input contract violation.
        reason: String,
    },
    /// A public duration violated an operation bound.
    #[error("invalid duration `{field}`: {reason}")]
    InvalidDuration {
        /// Name of the invalid duration.
        field: &'static str,
        /// Duration contract violation.
        reason: String,
    },
    /// An optimistic action guard rejected before mutation.
    #[error("presented state changed from `{expected_state_hash}` to `{current_state_hash}`")]
    StaleState {
        /// Presented-state hash required by the caller.
        expected_state_hash: String,
        /// Presented-state hash observed by the actor.
        current_state_hash: String,
        /// Actor sequence boundaries proving rejection preceded mutation.
        boundaries: Box<ActorBoundaries>,
        /// Retry constraint established by the actor evidence.
        retry_safety: RetrySafety,
    },
    /// Actor exclusivity rejected an action before mutation.
    #[error("actor action is busy: {message}")]
    ActionBusy {
        /// Safe actor-busy detail.
        message: String,
    },
    /// An input action retained a known applied prefix.
    #[error("partial input stopped after a bounded applied prefix")]
    PartialInput {
        /// Bounded per-atom input failure evidence.
        details: InputSequenceFailureDetails,
        /// Retry constraint established by the actor evidence.
        retry_safety: RetrySafety,
        /// Whether delivery prevents establishing an exact applied prefix.
        uncertain: bool,
    },
    /// Input completed but projection of its terminating frame failed.
    #[error(
        "action `{request_id}` applied all input but could not project its terminating frame: {source}"
    )]
    ActionTerminatingFrame {
        /// Identity of the applied action.
        request_id: Uuid,
        /// Actor sequence boundaries enclosing input application.
        boundaries: Box<ActorBoundaries>,
        /// Whether every requested input atom was applied.
        input_fully_applied: bool,
        /// Retry constraint established by the actor evidence.
        retry_safety: RetrySafety,
        #[source]
        /// Rich-frame projection failure.
        source: ScreenFrameError,
    },
    /// A strict action postcondition returned incomplete evidence.
    #[error("action postcondition did not complete successfully")]
    PostconditionIncomplete {
        /// Complete incomplete action result retained for inspection.
        result: Box<ActionResult>,
    },
    /// A strict stable capture returned incomplete evidence.
    #[error("stable capture did not reach the requested stable interval")]
    StableCaptureIncomplete {
        /// Complete incomplete capture retained for inspection.
        result: Box<StableScreenCapture>,
    },
    /// An SDK invariant or caller contract was invalid.
    #[error("invalid SDK operation: {reason}")]
    InvalidOperation {
        /// SDK invariant or caller contract violation.
        reason: String,
    },
}

/// SDK result type.
pub type Result<T> = StdResult<T, Error>;
