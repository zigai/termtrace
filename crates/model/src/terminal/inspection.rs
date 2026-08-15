//! Stable agent-facing inspection outcome contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::control::{ScreenPredicate, ScreenWaitQuality},
    terminal::screen::ScreenFrame,
};

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
