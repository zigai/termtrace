//! Timed canonical-event playback for the native Ghostty capture process.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::terminal::replay::SourceRange;

/// Clock handshake written before playback begins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaybackTiming {
    /// Wall-clock time corresponding to source-range start (or event time zero).
    pub replay_origin_unix_ns: u128,
    /// Playback speed multiplier (`1` for native timing).
    pub speed: u32,
    /// Selected half-open source interval, absent for full-session playback.
    pub source_range: Option<SourceRange>,
    /// Exact endpoint event for single-state playback.
    pub endpoint_event_seq: Option<u64>,
    /// Inclusive byte offset applied within the endpoint output event.
    pub endpoint_event_byte_offset: Option<u64>,
    /// Whether source-time sleeps were disabled.
    pub instant: bool,
}

/// Structured failure handshake written before playback exits unsuccessfully.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaybackFailure {
    /// Human-readable root cause from the playback process.
    pub message: String,
}
