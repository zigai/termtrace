//! Coherent errors exposed by the blocking SDK boundary.

use crate::{actions::ActOutcome, screen::StableScreenCapture};
use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};
use termtrace_control::Error as ControlError;
use termtrace_model::{
    protocol::{
        control::ControlErrorDetails,
        mutation::{MutationEffect, Retry},
    },
    terminal::screen::ScreenFrameError,
};
use thiserror::Error;

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
    /// The presented-state guard no longer matched, so nothing was applied.
    StaleState,
    /// Actor was busy processing another action.
    ActionBusy,
    /// Partial input failure with known prefix.
    PartialInput,
    /// Strict action postcondition was incomplete.
    PostconditionIncomplete,
    /// Strict stable capture was incomplete.
    StableCaptureIncomplete,
    /// Caller contract or SDK invariant violation.
    InvalidOperation,
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
    /// A strict action postcondition returned incomplete evidence.
    #[error("action postcondition did not complete successfully")]
    PostconditionIncomplete {
        /// Complete incomplete action result retained for inspection.
        result: Box<ActOutcome>,
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
                ControlError::Rejected { code, .. } => match code.as_str() {
                    "stale_observation" => ErrorKind::StaleState,
                    "action_busy" | "input_sequence_busy" => ErrorKind::ActionBusy,
                    "input_sequence_incomplete" => ErrorKind::PartialInput,
                    "invalid_screen_region" | "screen_frame_resource_limit" => {
                        ErrorKind::ScreenFrame
                    }
                    _ => ErrorKind::ControlRejected,
                },
            },
            Self::Evidence(_) => ErrorKind::Evidence,
            Self::Engine(_) => ErrorKind::Engine,
            Self::Export(_) => ErrorKind::Export,
            Self::Media(_) => ErrorKind::Media,
            Self::Filesystem { .. } => ErrorKind::Filesystem,
            Self::InvalidJson { .. } => ErrorKind::InvalidJson,
            Self::ScreenFrame(_) => ErrorKind::ScreenFrame,
            Self::PostconditionIncomplete { .. } => ErrorKind::PostconditionIncomplete,
            Self::StableCaptureIncomplete { .. } => ErrorKind::StableCaptureIncomplete,
            Self::InvalidOperation { .. } => ErrorKind::InvalidOperation,
        };
    }

    /// Stable code: the actor's rejection code when it rejected the request, otherwise the
    /// code of [`Self::kind`].
    #[must_use]
    pub fn code(&self) -> &str {
        if let Self::Control(ControlError::Rejected { code, .. }) = self {
            return code;
        }
        return self.kind().as_str();
    }

    /// Actor-consistent context attached to a rejection.
    #[must_use]
    pub fn details(&self) -> Option<&ControlErrorDetails> {
        let Self::Control(ControlError::Rejected { details, .. }) = self else {
            return None;
        };

        return details.as_deref();
    }

    /// What the failed request did to the session.
    #[must_use]
    pub const fn effect(&self) -> MutationEffect {
        return match self {
            Self::Control(error) => error.effect(),
            Self::PostconditionIncomplete { .. } => MutationEffect::Applied,
            Self::Evidence(_)
            | Self::Engine(_)
            | Self::Export(_)
            | Self::Media(_)
            | Self::Filesystem { .. }
            | Self::InvalidJson { .. }
            | Self::ScreenFrame(_)
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

impl ErrorKind {
    /// Stable snake-case spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        return match self {
            Self::Evidence => "evidence",
            Self::Engine => "engine",
            Self::Export => "export",
            Self::Media => "media",
            Self::Filesystem => "filesystem",
            Self::InvalidJson => "invalid_json",
            Self::InvalidControlProtocol => "invalid_control_protocol",
            Self::ControlUnavailable => "control_unavailable",
            Self::ControlTimeout => "control_timeout",
            Self::ControlTransport => "control_transport",
            Self::ControlRejected => "control_rejected",
            Self::ScreenFrame => "screen_frame",
            Self::StaleState => "stale_state",
            Self::ActionBusy => "action_busy",
            Self::PartialInput => "partial_input",
            Self::PostconditionIncomplete => "postcondition_incomplete",
            Self::StableCaptureIncomplete => "stable_capture_incomplete",
            Self::InvalidOperation => "invalid_operation",
        };
    }
}

/// SDK result type with defaulted error parameter.
pub type Result<T, E = Error> = StdResult<T, E>;
