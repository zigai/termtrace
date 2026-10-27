//! Stable-capture requests and outcomes, and plain text of rich frames.

use crate::{actions::Reaction, defaults};
use schemars::JsonSchema;
use serde::Serialize;
use std::time::Duration;
use termtrace_model::{
    protocol::control::{ScreenPredicate, ScreenWaitQuality, StableCaptureQuality},
    terminal::{
        inspection::ScreenWaitOutcome,
        screen::{ScreenFrame, ScreenFrameContent, ScreenProjectionRequest},
    },
    value::ByteString,
};

/// Options for one exact stable rich-screen capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableCaptureOptions {
    /// Required interval during which the presented state remains unchanged.
    pub stable_for: Duration,
    /// How long the actor waits for stability; the client allows the command's response slack.
    pub timeout: Duration,
    /// Detail and region requested for the captured frame.
    pub projection: ScreenProjectionRequest,
    /// Whether to return an unsettled capture instead of an error.
    pub allow_incomplete: bool,
    /// Reaction required before the stable interval starts.
    pub reaction: Option<Reaction>,
}

impl Default for StableCaptureOptions {
    fn default() -> Self {
        return Self {
            stable_for: defaults::IDLE_STABLE_FOR,
            timeout: defaults::WAIT_TIMEOUT,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
            reaction: None,
        };
    }
}

/// Atomic stable-screen outcome retaining the exact frame.
///
/// The frame is not serialized; bindings attach it as a native frame object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[schemars(extend("x-termtrace-frame" = true))]
pub struct StableScreenCapture {
    /// Why the capture returned; `Completed` when the session had already completed.
    pub quality: StableCaptureQuality,
    /// Exact frame projected before waiter ordering was released, or the final frame.
    #[serde(skip)]
    pub frame: ScreenFrame,
    /// First actor timestamp at which this exact state was observed.
    pub first_observed_monotonic_ns: u64,
    /// Actor timestamp of the returned snapshot.
    pub observed_monotonic_ns: u64,
    /// Requested unchanged interval.
    pub stable_for_ns: u64,
    /// Zstandard-compressed canonical JSON of the captured live state; `None` once completed.
    pub semantic_state_zstd: Option<ByteString>,
}

/// Screen wait evidence with the derived satisfaction verdict and without its frame, which
/// bindings attach as a native frame object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[schemars(extend("x-termtrace-frame" = true))]
pub struct ScreenWaitResult {
    /// Predicate evaluated by the actor.
    pub predicate: ScreenPredicate,
    /// Whether the predicate matched before termination.
    pub matched: bool,
    /// Whether the predicate held on a frame that met the requested stability.
    pub satisfied: bool,
    /// Closed completion classification.
    pub quality: ScreenWaitQuality,
    /// First matching actor timestamp, when matched.
    pub first_match_monotonic_ns: Option<u64>,
    /// Actor timestamp of the returned frame.
    pub observed_monotonic_ns: u64,
    /// Requested stability duration.
    pub stable_for_ns: u64,
}

impl From<&ScreenWaitOutcome> for ScreenWaitResult {
    fn from(outcome: &ScreenWaitOutcome) -> Self {
        return Self {
            predicate: outcome.predicate.clone(),
            matched: outcome.matched,
            satisfied: outcome.is_satisfied(),
            quality: outcome.quality,
            first_match_monotonic_ns: outcome.first_match_monotonic_ns,
            observed_monotonic_ns: outcome.observed_monotonic_ns,
            stable_for_ns: outcome.stable_for_ns,
        };
    }
}

/// Extract normalized plain text from any rich screen frame variant.
#[must_use]
pub fn frame_text(frame: &ScreenFrame) -> String {
    return match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        ScreenFrameContent::None => String::new(),
    };
}
