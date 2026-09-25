//! Failures while reaching or observing a live session.

use std::{io::Error as IoError, path::PathBuf, result::Result as StdResult};
use termtrace_model::{
    execution::session::SessionPhase,
    protocol::{
        control::ControlErrorDetails,
        mutation::{MutationEffect, Retry},
    },
};
use thiserror::Error;

/// Failure to reach, exchange with, or observe a session's recorder.
#[derive(Debug, Error)]
pub enum Error {
    /// The live actor could not be reached; `phase` says what the evidence shows instead.
    #[error("live control is unavailable for session `{session}` ({})", phase.as_str())]
    Unavailable {
        /// Canonical session directory.
        session: PathBuf,
        /// Session phase observed from evidence after the connection failed.
        phase: SessionPhase,
        #[source]
        /// Connection failure reported by the operating system.
        source: IoError,
    },
    /// An exchange step exceeded its deadline.
    #[error(
        "live control timed out during `{step}` for session `{session}` after {timeout_ms} ms"
    )]
    Timeout {
        /// Canonical session directory.
        session: PathBuf,
        /// Transport step that exceeded its deadline.
        step: &'static str,
        /// Requested deadline in milliseconds.
        timeout_ms: u64,
        /// What the request may have done, given how far it got.
        effect: MutationEffect,
        #[source]
        /// Underlying timeout or transport failure.
        source: IoError,
    },
    /// Control transport failed with possibly uncertain delivery.
    #[error("live control transport failed during `{step}` for session `{session}`: {source}")]
    Transport {
        /// Canonical session directory.
        session: PathBuf,
        /// Transport step that failed.
        step: &'static str,
        /// What the request may have done, given how far it got.
        effect: MutationEffect,
        #[source]
        /// Underlying transport failure.
        source: IoError,
    },
    /// A control message violated framing or correlation rules.
    #[error("invalid control protocol message: {reason}")]
    InvalidProtocol {
        /// Bounded protocol validation failure.
        reason: String,
    },
    /// The actor's answer to a dispatched request was unreadable or uncorrelated.
    #[error("invalid control response: {reason}")]
    InvalidResponse {
        /// Bounded response validation failure.
        reason: String,
        /// What the request may have done, since it reached the actor.
        effect: MutationEffect,
    },
    /// A request cannot be sent as specified.
    #[error("invalid control request: {reason}")]
    InvalidRequest {
        /// Request contract violation.
        reason: String,
    },
    /// A session file or response was not valid JSON for its schema.
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
    /// A session file could not be read.
    #[error("filesystem operation failed for `{path}`")]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        #[source]
        /// Underlying I/O failure.
        source: IoError,
    },
    /// Published completion names a different session than the manifest.
    #[error("completion evidence of session `{session}` names a different session identity")]
    CompletionIdentity {
        /// Canonical session directory.
        session: PathBuf,
    },
    /// The actor rejected a well-formed request.
    #[error("live session rejected control request `{code}`: {message}")]
    Rejected {
        /// Stable actor rejection code.
        code: String,
        /// Safe actor rejection detail.
        message: String,
        /// What the request did before it failed, as stated by the actor.
        effect: MutationEffect,
        /// Actor-consistent context attached to the rejection.
        details: Option<Box<ControlErrorDetails>>,
    },
}

impl Error {
    /// Whether the actor could not be reached or stopped answering mid-exchange.
    #[must_use]
    pub const fn is_connection_failure(&self) -> bool {
        return matches!(
            self,
            Self::Unavailable { .. } | Self::Timeout { .. } | Self::Transport { .. }
        );
    }

    /// What the failed request did: stated by the actor, or implied by how far
    /// the transport got.
    #[must_use]
    pub const fn effect(&self) -> MutationEffect {
        return match self {
            Self::Timeout { effect, .. }
            | Self::Transport { effect, .. }
            | Self::InvalidResponse { effect, .. }
            | Self::Rejected { effect, .. } => *effect,
            Self::Unavailable { .. }
            | Self::InvalidProtocol { .. }
            | Self::InvalidRequest { .. }
            | Self::InvalidJson { .. }
            | Self::Filesystem { .. }
            | Self::CompletionIdentity { .. } => MutationEffect::NotApplied,
        };
    }

    /// What the caller may safely do next, derived from [`Self::effect`].
    #[must_use]
    pub const fn retry(&self) -> Retry {
        return self.effect().retry();
    }
}

/// Control result type.
pub type Result<T, E = Error> = StdResult<T, E>;
