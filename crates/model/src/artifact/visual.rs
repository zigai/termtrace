//! Native visual artifact value and report contracts.

use crate::{
    artifact::metadata::ArtifactMember,
    execution::session::CaptureTarget,
    terminal::replay::{SourceRange, StateSelection},
    value::geometry::TerminalGeometryReport,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Native visual artifact directory name.
pub const VISUAL_DIRECTORY: &str = "visual";
/// Current native visual manifest schema version.
pub const VISUAL_MANIFEST_VERSION: u32 = 5;
/// Exact effective Ghostty configuration captured as native visual evidence.
pub const GHOSTTY_EFFECTIVE_CONFIG_NAME: &str = "ghostty-effective-config.txt";

/// Native capture configuration.
#[derive(Clone, Debug)]
pub struct CaptureOptions {
    /// Source session directory, or new session destination for original execution capture.
    pub session_directory: PathBuf,
    /// Xvfb/capture width in pixels.
    pub width: u16,
    /// Xvfb/capture height in pixels.
    pub height: u16,
    /// Encoded video frame rate.
    pub fps: u16,
    /// Optional inclusive source offset.
    pub start_monotonic_ns: Option<u64>,
    /// Optional exclusive source offset.
    pub end_monotonic_ns: Option<u64>,
    /// Semantic index retention mode to consume.
    pub state_selection: StateSelection,
    /// Capture-time exact-grid viewport search evidence.
    pub geometry_solver: Option<GeometrySolveReport>,
    /// Video or exact-state screenshot capture mode.
    pub kind: CaptureKind,
    /// Optional caller-selected artifact directory.
    pub destination: Option<PathBuf>,
    /// Optional display scaling factor (e.g. 1.0, 1.5, 2.0).
    pub scale: Option<f64>,
    /// Optional display DPI (e.g. 96, 144, 192).
    pub dpi: Option<u32>,
}

impl CaptureOptions {
    /// Configure display DPI explicitly.
    #[must_use]
    pub fn with_dpi(mut self, dpi: u32) -> Self {
        self.dpi = Some(dpi);

        return self;
    }

    /// Configure display scaling factor explicitly.
    #[must_use]
    pub fn with_scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);

        return self;
    }
}

/// Native capture product and exact endpoint when applicable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureKind {
    /// Timed native video plus final frame.
    Video,
    /// Direct native frame after exact-state replay.
    Screenshot {
        /// Selected semantic state identifier.
        state_id: u64,
        /// Selected complete semantic state hash.
        state_hash: String,
        /// Canonical endpoint event.
        event_seq: u64,
        /// Inclusive byte offset for output-event endpoints.
        event_byte_offset: Option<u64>,
    },
}

/// Completed native capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOutcome {
    /// Atomically committed visual directory.
    pub directory: PathBuf,
    /// Video path.
    pub video: Option<PathBuf>,
    /// Final native screenshot path.
    pub screenshot: PathBuf,
}

/// One viewport and measured grid considered by the exact-grid solver.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeometryProbeAttempt {
    /// Whole native viewport supplied to Ghostty.
    pub target: CaptureTarget,
    /// Grid, cell, and text-area dimensions reported by Ghostty.
    pub report: TerminalGeometryReport,
}

/// Bounded capture-time search proving the selected viewport fits the canonical grid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeometrySolveReport {
    /// Required canonical columns.
    pub required_cols: u16,
    /// Required canonical rows.
    pub required_rows: u16,
    /// Ordered probe decisions, including the independently measured final target.
    pub attempts: Vec<GeometryProbeAttempt>,
    /// Exact-grid selected viewport.
    pub selected: CaptureTarget,
}

/// State-to-video mapping entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VideoMapEntry {
    /// Semantic state identifier.
    pub state_id: u64,
    /// Semantic state hash.
    pub state_hash: String,
    /// Absolute canonical monotonic semantic timestamp.
    pub monotonic_ns: u64,
    /// Approximate zero-based video frame; absent for suppressed states.
    pub frame: Option<u64>,
    /// Whether Ghostty was allowed to present this state.
    pub presented: bool,
    /// Actual X11 acquisition timestamp of the correlated frame, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition_unix_ns: Option<u128>,
    /// Correlation is based on clocks; it does not prove terminal presentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<String>,
}

/// Provenance and probe evidence for native visual artifacts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisualManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Source session identifier.
    pub session_id: Uuid,
    /// Terminal hash of the source event log.
    pub source_event_hash: Option<String>,
    /// Selected half-open source interval, absent for a full-session capture.
    pub source_range: Option<SourceRange>,
    /// Semantic index retention mode used for frame mapping.
    pub state_selection: StateSelection,
    /// Parent-side Ghostty geometry preflight at the capture viewport.
    pub geometry_report: TerminalGeometryReport,
    /// Video frame rate; zero denotes compositor-controlled variable timing for Wayland originals.
    pub fps: u16,
    /// Capture width.
    pub width: u16,
    /// Capture height.
    pub height: u16,
    /// First X11 acquisition, first Wayland collector receipt, or screenshot request wall-clock time.
    pub capture_started_unix_ns: u128,
    /// Playback event-time origin.
    pub replay_origin_unix_ns: u128,
    /// BLAKE3 of the exact effective configuration captured as a manifest member.
    pub effective_config_hash: String,
    /// Installed Ghostty version report.
    pub ghostty_version: String,
    /// `FFmpeg` version first line.
    pub ffmpeg_version: Option<String>,
    /// Exact headless launch overrides.
    pub ghostty_overrides: Vec<String>,
    /// Parsed `FFprobe` video evidence.
    pub video_probe: Option<serde_json::Value>,
    /// Parsed `FFprobe` screenshot evidence.
    pub screenshot_probe: serde_json::Value,
    /// Whole-file integrity and classification for every payload member.
    pub members: Vec<ArtifactMember>,
    /// `video`, `screenshot`, or `original_execution`.
    pub capture_kind: String,
    /// Exact selected state identifier for direct screenshots.
    pub selected_state_id: Option<u64>,
    /// Exact selected semantic state hash for direct screenshots.
    pub selected_state_hash: Option<String>,
    /// Exact endpoint event for direct screenshots.
    pub endpoint_event_seq: Option<u64>,
    /// Inclusive endpoint byte offset within an output event.
    pub endpoint_event_byte_offset: Option<u64>,
    /// Parser barrier and native frame-stability guarantee label.
    pub presentation_evidence: String,
    /// Ordered native frame samples used for stability evidence.
    pub frame_samples: Vec<NativeFrameSample>,
    /// Honest native resource guarantee level.
    pub guarantee_level: String,
}

/// One directly captured native frame sample.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeFrameSample {
    /// Zero-based sample number.
    pub sample: u16,
    /// Capture wall-clock timestamp.
    pub captured_unix_ns: u128,
    /// BLAKE3 of exact PNG bytes.
    pub blake3: String,
    /// Whether this sample equals its immediate predecessor.
    pub same_as_previous: bool,
}

/// Resolve a deterministic visual directory for a range and index mode.
pub fn visual_directory_name_for(
    source_range: Option<SourceRange>,
    state_selection: StateSelection,
) -> String {
    let base = match state_selection {
        StateSelection::All => VISUAL_DIRECTORY.to_owned(),
        StateSelection::Presented => format!("{VISUAL_DIRECTORY}-presented"),
    };

    return source_range.map_or_else(
        || return base.clone(),
        |range| return format!("{base}-{}", range.artifact_suffix()),
    );
}
