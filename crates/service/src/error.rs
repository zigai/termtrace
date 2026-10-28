//! Coherent errors exposed by the blocking SDK boundary.

use crate::{actions::ActOutcome, screen::StableScreenCapture};
use std::{
    error::Error as StdError, io::Error as IoError, path::PathBuf, result::Result as StdResult,
};
pub use termtrace_control::Error as ControlError;
use termtrace_model::protocol::management::{FailureContext, ManagementFailure, TmuxFailureFacts};
use termtrace_model::{
    artifact::original::OriginalCaptureFacts,
    execution::visual_profile::VisualProfileFailure,
    protocol::{
        control::{CommandError, ControlErrorDetails},
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
    /// Aggregate tmux operation failed; inspect the aggregate before attempting another operation.
    Tmux,
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
    /// A recorder failed to start or become ready.
    RecorderStartup,
    /// An original capture did not publish; the application may have run.
    OriginalCaptureFailed,
    /// An original capture did not reach a final state before the caller's deadline.
    OriginalFinalizationTimeout,
    /// Caller contract or SDK invariant violation.
    InvalidOperation,
}

/// Failures translated at the SDK boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// Live control transport, rejection, or phase observation failed.
    #[error(transparent)]
    Control(#[from] termtrace_control::Error),
    /// A control command broke a request-shape rule before it was sent.
    #[error(transparent)]
    Command(#[from] CommandError),
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
    /// Aggregate management failed; already completed track effects are not rolled back.
    #[error(transparent)]
    Tmux(#[from] termtrace_tmux::Error),
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
    /// A recorder failed to start, exited, or missed its readiness deadline.
    #[error("{reason}")]
    RecorderStartup {
        /// What happened and where the recorder's diagnostics are.
        reason: String,
    },
    /// A later observation failed after this request's mutation effect was proven.
    /// Classification, diagnostics and the source remain those of the underlying failure.
    #[error("{source}")]
    MutationContext {
        /// Proven effect of this request, not of the later completion/query operation.
        effect: MutationEffect,
        /// Original completion, reaping or observer failure, without actor-error fabrication.
        #[source]
        source: Box<Error>,
    },
    /// Non-actor orchestration failed after the recorded workflow progress.
    #[error("{source}; inspect management facts; no automatic retry")]
    ManagementContext {
        /// Actual workflow progress, not input atoms or an actor effect.
        facts: Box<ManagementFailure>,
        /// Original error, retaining classification, code and source chain.
        #[source]
        source: Box<Error>,
    },
    /// An original capture did not publish its artifact. Canonical completion, when valid, and
    /// the paths that survived bounded cleanup are in `facts`.
    #[error("original capture failed ({}): {}", .facts.code.map_or("unknown", |code| return code.code()), .facts.reason)]
    OriginalCaptureFailed {
        /// Lifecycle facts; no actor effect.
        facts: Box<OriginalCaptureFacts>,
    },
    /// An original capture did not reach a final state before the deadline. Finish, wait or
    /// force-close again; nothing was rolled back.
    #[error("original capture did not finish in time (state {:?}): {}", .facts.state, .facts.reason)]
    OriginalFinalizationTimeout {
        /// Lifecycle facts observed at the deadline; no actor effect.
        facts: Box<OriginalCaptureFacts>,
    },
    /// An SDK invariant or caller contract was invalid.
    #[error("invalid SDK operation: {reason}")]
    InvalidOperation {
        /// SDK invariant or caller contract violation.
        reason: String,
        /// Lower-level failure that exposed the violation, when one exists.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
}

impl Error {
    /// Return the machine-readable error kind.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        if let Self::Tmux(error) = self
            && let Some(source) = tmux_service_source(error)
        {
            return source.kind();
        }
        return match self {
            Self::MutationContext { source, .. } | Self::ManagementContext { source, .. } => {
                source.kind()
            }
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
            Self::Command(_) => ErrorKind::ControlRejected,
            Self::Evidence(_) => ErrorKind::Evidence,
            Self::Engine(termtrace_engine::Error::VisualProfile {
                failure: VisualProfileFailure::InvalidVisualProfile,
                ..
            }) => ErrorKind::InvalidOperation,
            Self::Engine(_) => ErrorKind::Engine,
            Self::Export(_) => ErrorKind::Export,
            Self::Media(_) => ErrorKind::Media,
            Self::Tmux(_) => ErrorKind::Tmux,
            Self::Filesystem { .. } => ErrorKind::Filesystem,
            Self::InvalidJson { .. } => ErrorKind::InvalidJson,
            Self::ScreenFrame(_) => ErrorKind::ScreenFrame,
            Self::PostconditionIncomplete { .. } => ErrorKind::PostconditionIncomplete,
            Self::StableCaptureIncomplete { .. } => ErrorKind::StableCaptureIncomplete,
            Self::RecorderStartup { .. } => ErrorKind::RecorderStartup,
            Self::OriginalCaptureFailed { .. } => ErrorKind::OriginalCaptureFailed,
            Self::OriginalFinalizationTimeout { .. } => ErrorKind::OriginalFinalizationTimeout,
            Self::InvalidOperation { .. } => ErrorKind::InvalidOperation,
        };
    }

    /// Stable code: the actor's rejection code when it rejected the request, otherwise the
    /// code of [`Self::kind`].
    #[must_use]
    pub fn code(&self) -> &str {
        if let Self::Tmux(error) = self
            && let Some(source) = tmux_service_source(error)
        {
            return source.code();
        }

        if let Self::MutationContext { source, .. } | Self::ManagementContext { source, .. } = self
        {
            return source.code();
        }

        if let Self::Control(ControlError::Rejected { code, .. }) = self {
            return code;
        }

        if let Self::Command(error) = self {
            return error.code();
        }

        if let Self::OriginalCaptureFailed { facts } = self
            && let Some(code) = facts.code
        {
            return code.code();
        }

        if let Self::Engine(termtrace_engine::Error::VisualProfile { failure, .. }) = self {
            return failure.code();
        }
        return self.kind().as_str();
    }

    /// Actor-consistent context attached to a rejection.
    #[must_use]
    pub fn details(&self) -> Option<&ControlErrorDetails> {
        if let Self::Tmux(error) = self
            && let Some(source) = tmux_service_source(error)
        {
            return source.details();
        }

        if let Self::MutationContext { source, .. } | Self::ManagementContext { source, .. } = self
        {
            return source.details();
        }

        let Self::Control(ControlError::Rejected { details, .. }) = self else {
            return None;
        };

        return details.as_deref();
    }

    /// Actor effect accessor. Management returns Unknown only for compatibility;
    /// it is not an actor request and has no actor retry guarantee. Use `failure_context`.
    /// No management step is represented as Partial input atoms.
    #[must_use]
    pub const fn effect(&self) -> MutationEffect {
        return match self {
            Self::MutationContext { effect, .. } => *effect,
            Self::Control(error) => error.effect(),
            Self::Tmux(_)
            | Self::ManagementContext { .. }
            | Self::OriginalCaptureFailed { .. }
            | Self::OriginalFinalizationTimeout { .. } => MutationEffect::Unknown,
            Self::PostconditionIncomplete { .. } => MutationEffect::Applied,
            Self::Command(_)
            | Self::Evidence(_)
            | Self::Engine(_)
            | Self::Export(_)
            | Self::Media(_)
            | Self::Filesystem { .. }
            | Self::InvalidJson { .. }
            | Self::ScreenFrame(_)
            | Self::StableCaptureIncomplete { .. }
            | Self::RecorderStartup { .. }
            | Self::InvalidOperation { .. } => MutationEffect::NotApplied,
        };
    }

    /// Actual actor-only effect, absent for a non-actor management workflow.
    /// Prefer this or `failure_context` over the legacy effect accessor.
    #[must_use]
    pub const fn actor_effect(&self) -> Option<MutationEffect> {
        if self.is_management() {
            return None;
        }
        return Some(self.effect());
    }

    /// Attach an effect proven by the caller's protocol acknowledgement, without changing
    /// the classification or content of a subsequent operational error.
    pub(crate) fn with_proven_effect(self, effect: MutationEffect) -> Self {
        return Self::MutationContext {
            effect,
            source: Box::new(self),
        };
    }

    /// Legacy retry accessor. For management this prohibits retry; use
    /// [`Self::failure_context`] for the explicitly non-actor inspect policy.
    /// Actor errors retain the admitted effect-to-retry mapping unchanged.
    #[must_use]
    pub const fn retry(&self) -> Retry {
        if self.is_management() {
            return Retry::DoNotRetry;
        }
        return self.effect().retry();
    }

    const fn is_management(&self) -> bool {
        return matches!(
            self,
            Self::ManagementContext { .. }
                | Self::Tmux(_)
                | Self::OriginalCaptureFailed { .. }
                | Self::OriginalFinalizationTimeout { .. }
        );
    }

    /// Attach explicitly scoped non-actor progress without altering the underlying error.
    pub fn with_management(self, facts: ManagementFailure) -> Self {
        return Self::ManagementContext {
            facts: Box::new(facts),
            source: Box::new(self),
        };
    }

    /// Shared failure projection used by Rust callers and both native facades.
    #[must_use]
    pub fn failure_context(&self) -> FailureContext {
        return match self {
            Self::ManagementContext { facts, .. } => FailureContext::management(*facts.clone()),
            Self::OriginalCaptureFailed { facts } | Self::OriginalFinalizationTimeout { facts } => {
                FailureContext::management(ManagementFailure::OriginalCapture(*facts.clone()))
            }
            Self::Tmux(error) => {
                FailureContext::management(error.management_facts().cloned().unwrap_or_else(|| {
                    return ManagementFailure::Tmux(TmuxFailureFacts::inspect("tmux"));
                }))
            }
            _ => FailureContext::actor(self.effect()),
        };
    }
}

fn tmux_service_source(error: &termtrace_tmux::Error) -> Option<&Error> {
    return match error {
        termtrace_tmux::Error::Management { source, .. } => tmux_service_source(source),
        termtrace_tmux::Error::TrackSource { source, .. } => source.downcast_ref::<Error>(),
        _ => None,
    };
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
            Self::Tmux => "tmux",
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
            Self::RecorderStartup => "recorder_startup",
            Self::OriginalCaptureFailed => "original_capture_failed",
            Self::OriginalFinalizationTimeout => "original_finalization_timeout",
            Self::InvalidOperation => "invalid_operation",
        };
    }
}

/// SDK result type with defaulted error parameter.
pub type Result<T, E = Error> = StdResult<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proven_effect_preserves_kind_code_details_display_and_source() {
        let frame: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/locators/parity-frame.json"
        ))
        .expect("frame");

        let details: ControlErrorDetails = serde_json::from_value(serde_json::json!({
            "current_presented_state_hash": "state", "boundaries": frame["source"]["boundaries"], "scope": "completion"
        })).expect("details");

        let original = Error::Control(ControlError::Rejected {
            code: "wait_failed".to_owned(),
            message: "completion observation failed".to_owned(),
            effect: MutationEffect::NotApplied,
            details: Some(Box::new(details.clone())),
        });

        let display = original.to_string();
        let contextual = original.with_proven_effect(MutationEffect::Applied);
        assert_eq!(contextual.kind(), ErrorKind::ControlRejected);
        assert_eq!(contextual.code(), "wait_failed");
        assert_eq!(contextual.details(), Some(&details));
        assert_eq!(contextual.to_string(), display);
        assert_eq!(contextual.effect(), MutationEffect::Applied);
        assert_eq!(contextual.retry(), Retry::DoNotRetry);

        let source = StdError::source(&contextual)
            .expect("original error")
            .downcast_ref::<Box<Error>>()
            .expect("boxed typed original source")
            .as_ref();
        assert_eq!(source.effect(), MutationEffect::NotApplied);
        assert_eq!(source.details(), Some(&details));
    }

    #[test]
    fn proven_effect_keeps_the_underlying_io_source_chain() {
        let original = Error::Filesystem {
            path: PathBuf::from("/completion"),
            source: IoError::from_raw_os_error(13),
        };
        let display = original.to_string();
        let contextual = original.with_proven_effect(MutationEffect::Applied);
        assert_eq!(contextual.kind(), ErrorKind::Filesystem);
        assert_eq!(contextual.code(), "filesystem");
        assert_eq!(contextual.to_string(), display);

        let original = StdError::source(&contextual).expect("original filesystem error");
        let io = original
            .source()
            .expect("original I/O cause")
            .downcast_ref::<IoError>()
            .expect("I/O cause");
        assert_eq!(io.raw_os_error(), Some(13));
    }

    #[test]
    fn unacknowledged_collision_and_uncertain_transport_keep_their_effects() {
        let collision = Error::Control(ControlError::Rejected {
            code: "request_id_collision".to_owned(),
            message: "collision".to_owned(),
            effect: MutationEffect::NotApplied,
            details: None,
        });
        assert_eq!(collision.effect(), MutationEffect::NotApplied);
        assert_eq!(collision.retry(), Retry::NewRequest);

        let transport = Error::Control(ControlError::InvalidResponse {
            reason: "response lost after dispatch".to_owned(),
            effect: MutationEffect::Unknown,
        });
        assert_eq!(transport.effect(), MutationEffect::Unknown);
        assert_eq!(transport.retry(), Retry::SameRequestId);
    }

    #[test]
    fn management_context_does_not_reuse_actor_retry_and_preserves_source() {
        let source = Error::Control(ControlError::InvalidResponse {
            reason: "lost actor reply".into(),
            effect: MutationEffect::Unknown,
        });
        assert_eq!(source.retry(), Retry::SameRequestId);

        let error =
            source.with_management(ManagementFailure::Tmux(TmuxFailureFacts::inspect("stop")));
        assert_eq!(error.kind(), ErrorKind::InvalidControlProtocol);
        assert_eq!(error.code(), "invalid_control_protocol");
        assert_eq!(error.retry(), Retry::DoNotRetry);
        assert_eq!(error.actor_effect(), None);
        assert!(matches!(
            error.failure_context(),
            FailureContext::Management { .. }
        ));
        assert!(StdError::source(&error).is_some());
    }
}
