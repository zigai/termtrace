//! Stable agent-facing inspection outcome contracts.

use crate::{
    protocol::control::{ScreenPredicate, ScreenWaitQuality},
    terminal::screen::ScreenFrame,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Successful raw-output wait evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaitOutcome {
    /// Event containing the end of the first observed match.
    pub event_seq: u64,
    /// End offset in the concatenated output stream.
    pub output_end_offset: u64,
    /// Monotonic timestamp of the containing event.
    pub monotonic_ns: u64,
}

/// Exact presented-screen wait evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenWaitOutcome {
    /// Predicate evaluated by the actor.
    pub predicate: ScreenPredicate,
    /// Whether the predicate matched before termination.
    pub matched: bool,
    /// First matching actor timestamp, when matched.
    pub first_match_monotonic_ns: Option<u64>,
    /// Actor timestamp of the returned frame.
    pub observed_monotonic_ns: u64,
    /// Requested stability duration.
    pub stable_for_ns: u64,
    /// Closed completion classification.
    pub quality: ScreenWaitQuality,
    /// Exact satisfying or terminating frame.
    pub frame: ScreenFrame,
}

impl ScreenWaitOutcome {
    /// Whether the predicate held on a frame that met the requested stability: settled
    /// while live, unsettled when no stability was requested, or a final screen that can no
    /// longer change because the child completed.
    #[must_use]
    pub const fn satisfied(&self) -> bool {
        return self.matched
            && matches!(
                self.quality,
                ScreenWaitQuality::Matched
                    | ScreenWaitQuality::MatchedUnsettled
                    | ScreenWaitQuality::ChildCompleted
                    | ScreenWaitQuality::Completed
            );
    }
}
