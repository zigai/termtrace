//! Coherent errors exposed by the blocking SDK boundary.

use crate::{actions::ActionResult, client::StableScreenCapture};
use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};
use termtrace_control::Error as ControlError;
use termtrace_model::{
    protocol::{
        control::InputSequenceFailureDetails,
        mutation::{MutationEffect, Retry},
    },
    terminal::{observation::ActorBoundaries, screen::ScreenFrameError},
};
use thiserror::Error;
use uuid::Uuid;

/// Machine-readable classification of SDK and service errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// Evidence storage or validation error.
    Evidence,
    /// Terminal engine or replay error.
    Engine,
    /// Artifact export error.
    Export,
    /// Native media capture or render error.
    Media,
    /// Filesystem error.
    Filesystem,
    /// Invalid JSON document error.
    InvalidJson,
    /// Control command rejected by session actor.
    ControlRejected,
    /// Session control endpoint is unavailable.
    ControlUnavailable,
    /// Control command timed out.
    ControlTimeout,
    /// Control transport socket error.
    ControlTransport,
    /// Control protocol format violation.
    InvalidControlProtocol,
    /// Rich screen frame projection failure.
    ScreenFrame,
    /// Action atom validation failure.
    InvalidAtom,
    /// Action guard rejected due to state hash mismatch.
    StaleState,
    /// Actor was busy processing another action.
    ActionBusy,
    /// Partial input failure with known prefix.
    PartialInput,
    /// Terminating frame projection failed.
    ActionTerminatingFrame,
    /// Strict action postcondition was incomplete.
    PostconditionIncomplete,
    /// Strict stable capture was incomplete.
    StableCaptureIncomplete,
    /// Caller contract or SDK invariant violation.
    InvalidOperation,
    /// Invalid duration argument.
    InvalidDuration,
}

/// Failures translated at the SDK boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// Live control transport, rejection, or phase observation failed.
    #[error(transparent)]
    Control(#[from] termtrace_control::Error),
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
        /// Applied prefix as stated by the actor.
        effect: MutationEffect,
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

impl Error {
    /// Return the machine-readable error kind.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        return match self {
            Self::Control(error) => match error {
                ControlError::Unavailable { .. } => ErrorKind::ControlUnavailable,
                ControlError::Timeout { .. } => ErrorKind::ControlTimeout,
                ControlError::Transport { .. } => ErrorKind::ControlTransport,
                ControlError::InvalidProtocol { .. } | ControlError::InvalidResponse { .. } => {
                    ErrorKind::InvalidControlProtocol
                }
                ControlError::InvalidRequest { .. } => ErrorKind::InvalidOperation,
                ControlError::InvalidJson { .. } => ErrorKind::InvalidJson,
                ControlError::Filesystem { .. } => ErrorKind::Filesystem,
                ControlError::CompletionIdentity { .. } => ErrorKind::Evidence,
                ControlError::Rejected { .. } => ErrorKind::ControlRejected,
            },
            Self::Evidence(_) => ErrorKind::Evidence,
            Self::Engine(_) => ErrorKind::Engine,
            Self::Export(_) => ErrorKind::Export,
            Self::Media(_) => ErrorKind::Media,
            Self::Filesystem { .. } => ErrorKind::Filesystem,
            Self::InvalidJson { .. } => ErrorKind::InvalidJson,
            Self::ScreenFrame(_) => ErrorKind::ScreenFrame,
            Self::InvalidAtom { .. } => ErrorKind::InvalidAtom,
            Self::InvalidDuration { .. } => ErrorKind::InvalidDuration,
            Self::StaleState { .. } => ErrorKind::StaleState,
            Self::ActionBusy { .. } => ErrorKind::ActionBusy,
            Self::PartialInput { .. } => ErrorKind::PartialInput,
            Self::ActionTerminatingFrame { .. } => ErrorKind::ActionTerminatingFrame,
            Self::PostconditionIncomplete { .. } => ErrorKind::PostconditionIncomplete,
            Self::StableCaptureIncomplete { .. } => ErrorKind::StableCaptureIncomplete,
            Self::InvalidOperation { .. } => ErrorKind::InvalidOperation,
        };
    }

    /// Return the stable string error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        return match self.kind() {
            ErrorKind::Evidence => "evidence",
            ErrorKind::Engine => "engine",
            ErrorKind::Export => "export",
            ErrorKind::Media => "media",
            ErrorKind::Filesystem => "filesystem",
            ErrorKind::InvalidJson => "invalid_json",
            ErrorKind::InvalidControlProtocol => "invalid_control_protocol",
            ErrorKind::ControlUnavailable => "control_unavailable",
            ErrorKind::ControlTimeout => "control_timeout",
            ErrorKind::ControlTransport => "control_transport",
            ErrorKind::ControlRejected => "control_rejected",
            ErrorKind::ScreenFrame => "screen_frame",
            ErrorKind::InvalidAtom => "invalid_atom",
            ErrorKind::InvalidDuration => "invalid_duration",
            ErrorKind::StaleState => "stale_state",
            ErrorKind::ActionBusy => "action_busy",
            ErrorKind::PartialInput => "partial_input",
            ErrorKind::ActionTerminatingFrame => "action_terminating_frame",
            ErrorKind::PostconditionIncomplete => "postcondition_incomplete",
            ErrorKind::StableCaptureIncomplete => "stable_capture_incomplete",
            ErrorKind::InvalidOperation => "invalid_operation",
        };
    }

    /// What the failed request did to the session.
    #[must_use]
    pub const fn effect(&self) -> MutationEffect {
        return match self {
            Self::Control(error) => error.effect(),
            Self::PartialInput { effect, .. } => *effect,
            Self::ActionTerminatingFrame { .. } | Self::PostconditionIncomplete { .. } => {
                MutationEffect::Applied
            }
            Self::Evidence(_)
            | Self::Engine(_)
            | Self::Export(_)
            | Self::Media(_)
            | Self::Filesystem { .. }
            | Self::InvalidJson { .. }
            | Self::ScreenFrame(_)
            | Self::InvalidAtom { .. }
            | Self::InvalidDuration { .. }
            | Self::StaleState { .. }
            | Self::ActionBusy { .. }
            | Self::StableCaptureIncomplete { .. }
            | Self::InvalidOperation { .. } => MutationEffect::NotApplied,
        };
    }

    /// What the caller may safely do next, derived from [`Self::effect`].
    #[must_use]
    pub const fn retry(&self) -> Retry {
        return self.effect().retry();
    }
}

/// SDK result type with defaulted error parameter.
pub type Result<T, E = Error> = StdResult<T, E>;
