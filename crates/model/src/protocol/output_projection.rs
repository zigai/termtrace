//! Stable contracts for explicitly lossy child-output projections.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable quality label for one child-output projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputProjectionQuality {
    /// Exact canonical output bytes without interpretation.
    ExactBytes,
    /// Control-stripped bytes from a stream that did not enter alternate screen.
    LossyLineOriented,
    /// Control stripping cannot represent a full-screen application's visible state.
    UnsuitableForFullScreen,
}

/// Hash-linked metadata for exact or control-stripped output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputProjectionReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Immutable canonical session identity.
    pub session_id: Uuid,
    /// Final validated canonical event hash.
    pub final_event_hash: String,
    /// Inclusive selected canonical event sequence.
    pub from_event_seq: u64,
    /// Exclusive selected canonical event sequence.
    pub to_event_seq: u64,
    /// `raw` or `strip_controls`.
    pub mode: String,
    /// Honest projection quality.
    pub projection_quality: OutputProjectionQuality,
    /// Exact selected canonical output byte count.
    pub source_byte_count: u64,
    /// BLAKE3 of exact selected canonical output bytes.
    pub source_blake3: String,
    /// Emitted byte count.
    pub output_byte_count: u64,
    /// BLAKE3 of emitted bytes.
    pub output_blake3: String,
    /// Bytes intentionally removed by the lossy projection.
    pub dropped_byte_count: u64,
    /// Whether a recognized alternate-screen control was observed.
    pub alternate_screen_observed: bool,
    /// Stable warning explaining the projection boundary.
    pub warning: Option<String>,
}
