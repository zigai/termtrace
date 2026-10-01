//! Stable-capture requests and outcomes, and plain text of rich frames.

use crate::defaults;
use std::time::Duration;
use termtrace_model::{
    protocol::control::StableCaptureQuality,
    terminal::screen::{ScreenFrame, ScreenFrameContent, ScreenProjectionRequest},
    value::ByteString,
};

/// Options for one exact stable rich-screen capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableCaptureOptions {
    /// Required interval during which the presented state remains unchanged.
    pub stable_for: Duration,
    /// How long the actor waits for stability; the client allows the command's response slack.
    pub timeout: Duration,
    /// Detail and region requested for the captured frame.
    pub projection: ScreenProjectionRequest,
    /// Whether to return an unsettled capture instead of an error.
    pub allow_incomplete: bool,
}

impl Default for StableCaptureOptions {
    fn default() -> Self {
        return Self {
            stable_for: defaults::IDLE_STABLE_FOR,
            timeout: defaults::WAIT_TIMEOUT,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        };
    }
}

/// Atomic stable-screen outcome retaining the exact frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableScreenCapture {
    /// Why the capture returned; `Completed` when the session had already completed.
    pub quality: StableCaptureQuality,
    /// Exact frame projected before waiter ordering was released, or the final frame.
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
