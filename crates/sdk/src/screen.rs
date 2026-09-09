//! Presented rich-screen observations and stable capture.

pub use crate::client::{Screen, StableScreenCapture};
use std::time::Duration;
use termtrace_model::terminal::screen::ScreenProjectionRequest;

/// Options for one exact stable rich-screen capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableCaptureOptions {
    /// Required interval during which the presented state remains unchanged.
    pub stable_for: Duration,
    /// Maximum time to wait for the stable capture.
    pub timeout: Duration,
    /// Detail and region requested for the captured frame.
    pub projection: ScreenProjectionRequest,
    /// Whether to return an incomplete capture instead of an error.
    pub allow_incomplete: bool,
}
