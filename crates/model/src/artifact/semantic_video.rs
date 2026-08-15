//! Deterministic semantic-state video scheduling and source materialization.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current semantic-video source format.
pub const SEMANTIC_VIDEO_VERSION: u32 = 2;

/// Fixed canvas used for every frame of one semantic video source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoCanvas {
    /// Largest terminal column count in the source execution.
    pub cols: u16,
    /// Largest terminal row count in the source execution.
    pub rows: u16,
    /// Deterministic semantic cell width.
    pub cell_width: u16,
    /// Deterministic semantic cell height.
    pub cell_height: u16,
    /// Encoded source width.
    pub width: u16,
    /// Encoded source height.
    pub height: u16,
    /// Stable resize/canvas policy identifier.
    pub policy: String,
}

/// Immutable semantic source description resolved during presentation planning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoSourcePlan {
    /// Source-plan schema version.
    pub version: u32,
    /// Canonical session path.
    pub session: PathBuf,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Terminal canonical event hash.
    pub source_event_hash: String,
    /// Inclusive source-time origin.
    pub start_monotonic_ns: u64,
    /// Inclusive completed-session endpoint used for the final frame hold.
    pub end_monotonic_ns: u64,
    /// Constant semantic source frame rate.
    pub fps: u16,
    /// Exact source frame count, including one final semantic-state frame.
    pub frame_count: u64,
    /// Fixed source canvas.
    pub canvas: SemanticVideoCanvas,
    /// Semantic renderer identity.
    pub renderer: String,
    /// Bundled deterministic font family.
    pub font_family: String,
    /// BLAKE3 identity of the bundled font bytes.
    pub font_hash: String,
    /// Stable state-to-frame scheduling policy.
    pub scheduling_policy: String,
}

/// One contiguous source-frame span rendered from one semantic state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticFrameSpan {
    /// Inclusive source frame.
    pub start_frame: u64,
    /// Exclusive source frame.
    pub end_frame: u64,
    /// Exact semantic state identifier.
    pub state_id: u64,
    /// Exact complete semantic state hash.
    pub state_hash: String,
    /// Canonical state timestamp.
    pub state_monotonic_ns: u64,
}

/// Provenance and scheduling result for one materialized semantic source video.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoSourceManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Exact immutable source plan.
    pub plan: SemanticVideoSourcePlan,
    /// Product truth layer.
    pub truth_layer: String,
    /// Number of presentation-eligible states in the selected interval.
    pub presented_state_count: u64,
    /// Number of presentation-eligible states owning at least one frame.
    pub rendered_state_count: u64,
    /// Number of presentation-eligible states suppressed by frame sampling.
    pub suppressed_state_count: u64,
    /// BLAKE3 of exact RGBA frames sent to the encoder.
    pub pixel_stream_hash: String,
    /// BLAKE3 of the lossless semantic source video.
    pub source_video_hash: String,
    /// First line of the exact FFmpeg build used.
    pub ffmpeg_version: String,
    /// Exact FFmpeg argument vector.
    pub ffmpeg_arguments: Vec<String>,
    /// Parsed FFprobe source-video evidence.
    pub video_probe: serde_json::Value,
}
