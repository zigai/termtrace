//! Non-actor management failures have no replay identifier or deduplication guarantee.

use super::mutation::{MutationEffect, Retry};
use crate::{
    artifact::original::OriginalCaptureFacts, execution::session::SessionCompletion,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a restart stopped; workflow phases are not input atoms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestartFailurePhase {
    /// Resolving the predecessor, before any proven completion.
    ResolvePredecessor,
    /// Predecessor stop was invoked; inspect it if completion was not returned.
    StoppingPredecessor,
    /// The exact predecessor completion was returned.
    PredecessorCompleted,
    /// Successor paths were allocated.
    SuccessorAllocated,
    /// The actual successor Child was spawned.
    SuccessorLaunched,
    /// The successor passed readiness.
    SuccessorReady,
    /// Immutable successor lineage was validated.
    LineageValidated,
    /// Compare-and-retarget failed; do not assume the alias still has its old owner.
    AliasRetarget,
}

/// Result of observing a spawned successor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuccessorReadiness {
    /// Live at readiness observation.
    Live,
    /// Completed at readiness observation.
    Completed,
    /// Exited before readiness.
    Exited,
    /// Readiness deadline elapsed; the successor may remain running.
    TimedOut,
    /// Readiness observation failed; inspect the source/logs.
    Failed,
}

/// Exact allocated paths and proven successor launch/readiness facts, not rollback claims.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RestartAttempt {
    /// Authoritative session-directory bytes.
    pub session: ByteString,
    /// Authoritative stdout-log path bytes.
    pub stdout_log: ByteString,
    /// Authoritative stderr-log path bytes.
    pub stderr_log: ByteString,
    /// True only after spawning the actual Child.
    pub launched: bool,
    /// Last actual readiness observation, absent before observing it.
    pub readiness: Option<SuccessorReadiness>,
}

/// Proven restart progress. No alias rollback or deduplicated replay is promised.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RestartFailureFacts {
    /// Alias used for this orchestration.
    pub alias: String,
    /// Last reached phase.
    pub phase: RestartFailurePhase,
    /// Exact resolved predecessor path, when resolution succeeded.
    pub predecessor_path: Option<ByteString>,
    /// Actual immutable completion returned by predecessor stop, never synthesized.
    pub predecessor_completion: Option<Box<SessionCompletion>>,
    /// Allocated successor/orphan locations and proven admission facts.
    pub successor: Option<RestartAttempt>,
}

/// Last known tmux management phase; none of these are input-atom counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxFailurePhase {
    /// No precise progress receipt is available; inspect the aggregate/server.
    InspectState,
    /// `StopRequested` was committed to the aggregate journal.
    StopRequested,
    /// Composite stop returned successfully.
    CompositeStopCompleted,
    /// Waiting for track completions.
    WaitingTracks,
    /// Settling aggregate lifecycle after shutdown attempts.
    Settling,
}

/// Actual tmux progress receipts, independent of the actor mutation/retry protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxFailureFacts {
    /// Management entrypoint, not an actor request identifier.
    pub action: String,
    /// Last known phase.
    pub phase: TmuxFailurePhase,
    /// Exact aggregate manifest path, when known.
    pub manifest: Option<ByteString>,
    /// Whether `StopRequested` is proven committed; None means no such receipt is available.
    pub stop_requested: Option<bool>,
    /// Tracks whose stop operation returned successfully (not a rollback guarantee).
    pub completed_track_stops: Vec<ByteString>,
    /// Tracks whose completion wait returned successfully.
    pub completed_track_waits: Vec<ByteString>,
    /// Pane shutdown commands attempted; command success is not asserted.
    pub pane_shutdown_attempted: Vec<String>,
    /// Whether integration disable was attempted, not whether it succeeded.
    pub integration_disable_attempted: bool,
}

impl TmuxFailureFacts {
    /// Conservative context for entrypoints without precise orchestration receipts.
    #[must_use]
    pub fn inspect(action: impl Into<String>) -> Self {
        return Self {
            action: action.into(),
            phase: TmuxFailurePhase::InspectState,
            manifest: None,
            stop_requested: None,
            completed_track_stops: Vec::new(),
            completed_track_waits: Vec::new(),
            pane_shutdown_attempted: Vec::new(),
            integration_disable_attempted: false,
        };
    }
}

/// Discriminator for actual non-actor management progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "facts", rename_all = "snake_case")]
pub enum ManagementFailure {
    /// Registry restart workflow facts.
    RegistryRestart(RestartFailureFacts),
    /// Tmux management workflow facts.
    Tmux(TmuxFailureFacts),
    /// Original-execution capture lifecycle facts; the application may have run.
    OriginalCapture(OriginalCaptureFacts),
}

/// Management has no automatic replay policy, regardless of nested actor effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagementGuidance {
    /// Inspect predecessor, successor/orphan, alias and aggregate before a deliberate new action.
    InspectNoAutomaticRetry,
}

/// Shared failure projection. Actor-only effect/retry never describe a multi-step workflow.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum FailureContext {
    /// An actual actor request or its existing local admission/transport classification.
    Actor {
        /// The admitted actor effect.
        effect: MutationEffect,
        /// The unchanged actor-only retry rule.
        retry: Retry,
    },
    /// Non-actor management facts; no request-ID replay is available.
    Management {
        /// Explicit inspect/no-automatic-retry guidance.
        guidance: ManagementGuidance,
        /// Proven progress and exact paths.
        management: Box<ManagementFailure>,
    },
}

impl FailureContext {
    /// Use the single admitted actor mapping, never rederive it in native adapters.
    #[must_use]
    pub const fn actor(effect: MutationEffect) -> Self {
        return Self::Actor {
            effect,
            retry: effect.retry(),
        };
    }

    /// Scope a management failure without claiming an actor effect.
    #[must_use]
    pub fn management(facts: ManagementFailure) -> Self {
        return Self::Management {
            guidance: ManagementGuidance::InspectNoAutomaticRetry,
            management: Box::new(facts),
        };
    }
}
