//! Stable semantic replay records, selectors, and source ranges.

use crate::{
    artifact::metadata::ArtifactMember,
    execution::{event::EventLogPosition, event_log::CanonicalBoundary},
    terminal::state::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, ModeState, SemanticState,
        TerminalMetadata,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Pinned Ghostty source revision used by the semantic adapter.
pub const GHOSTTY_REVISION: &str = "9781c0afc8225510e33127466f0815eb51086d7d";
/// Derived artifact format version.
pub const STATE_FORMAT_VERSION: u32 = 11;
/// Replay-index member listing the retained semantic states.
pub const STATE_INDEX: &str = "state-index.jsonl";

/// An exact presentation-eligible endpoint within a validated canonical prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayEndpoint {
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Commitment to the source session manifest.
    pub session_manifest_hash: String,
    /// Whether the selected source has validated durable completion.
    pub source_complete: bool,
    /// Exact source prefix that was validated during resolution.
    pub boundary: CanonicalBoundary,
    /// Stream position of the endpoint state.
    pub state_id: u64,
    /// Hash of the complete normalized semantic state at the endpoint.
    pub state_hash: String,
    /// Canonical viewport columns at the endpoint.
    pub cols: u16,
    /// Canonical viewport rows at the endpoint.
    pub rows: u16,
    /// Final emitted event; absent for the initial blank state.
    pub event_seq: Option<u64>,
    /// Inclusive output byte offset when the endpoint is an output event.
    pub event_byte_offset: Option<u64>,
    /// Semantic source timestamp when known.
    pub monotonic_ns: Option<u64>,
}

/// A non-empty half-open interval on the canonical monotonic timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRange {
    /// Inclusive source timestamp in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive source timestamp in nanoseconds.
    pub end_monotonic_ns: u64,
}

impl SourceRange {
    /// Duration of the selected source interval.
    pub fn duration_ns(self) -> u64 {
        return self.end_monotonic_ns - self.start_monotonic_ns;
    }

    /// Whether a timestamp belongs to this half-open interval.
    pub fn contains(self, monotonic_ns: u64) -> bool {
        return monotonic_ns >= self.start_monotonic_ns && monotonic_ns < self.end_monotonic_ns;
    }

    /// Stable suffix used by range-specific no-replace artifact directories.
    pub fn artifact_suffix(self) -> String {
        return format!(
            "range-{}-{}",
            self.start_monotonic_ns, self.end_monotonic_ns
        );
    }
}

/// Semantic states retained in one derived index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateSelection {
    /// List every boundary state, including those inside synchronized output.
    #[default]
    All,
    /// List every boundary state eligible for presentation by a native terminal.
    Presented,
}

/// Policy for one semantic indexing operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IndexOptions {
    /// Optional inclusive source offset.
    pub start_monotonic_ns: Option<u64>,
    /// Optional exclusive source offset.
    pub end_monotonic_ns: Option<u64>,
    /// Permit a hash-addressed snapshot when completion is absent.
    pub live_snapshot: bool,
    /// Semantic states retained in artifacts.
    pub state_selection: StateSelection,
    /// Optional retention stride in monotonic nanoseconds (e.g. 1/FPS increments for video
    /// indexing): only the last presented state of each stride is listed in the state index.
    pub sampling_stride_ns: Option<u64>,
}

impl IndexOptions {
    /// Configure video indexing options with a sampling stride for the given FPS.
    #[must_use]
    pub fn for_video(fps: u32) -> Self {
        let stride_ns = if fps > 0 { 1_000_000_000_u64 / u64::from(fps) } else { 0 };

        return Self {
            state_selection: StateSelection::All,
            sampling_stride_ns: (stride_ns > 0).then_some(stride_ns),
            ..Self::default()
        };
    }

    /// Configure options with an explicit sampling stride in nanoseconds.
    #[must_use]
    pub fn with_sampling_stride(mut self, stride_ns: u64) -> Self {
        self.sampling_stride_ns = (stride_ns > 0).then_some(stride_ns);
        return self;
    }
}

/// Fine-grained changes for one viewport row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RowDiff {
    /// Zero-based row.
    pub row: u16,
    /// Whether the row ends in a native soft wrap.
    pub soft_wrapped: bool,
    /// Whether the row continues a native soft wrap from its predecessor.
    pub wrap_continuation: bool,
    /// Cells whose complete semantic value changed.
    pub cells: Vec<CellState>,
}

/// Delta from the previous state of the same replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateDelta {
    /// New `(cols, rows)` when geometry changed.
    pub geometry: Option<(u16, u16)>,
    /// New cursor when it changed.
    pub cursor: Option<CursorState>,
    /// New terminal modes when they changed.
    pub modes: Option<ModeState>,
    /// New colors and palette when they changed.
    pub colors: Option<ColorState>,
    /// New active SGR style when it changed.
    pub active_style: Option<CellStyle>,
    /// New terminal metadata when it changed.
    pub metadata: Option<TerminalMetadata>,
    /// Authoritative wrap and cell deltas by row.
    pub rows: Vec<RowDiff>,
}

/// The semantic state at one stream position.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateRecord {
    /// Stream position: the output bytes and applied resizes that produced this state.
    pub state_id: u64,
    /// Canonical event responsible for this state; absent for an initial or range-baseline state.
    pub event_seq: Option<u64>,
    /// Zero-based byte offset inside an output event.
    pub event_byte_offset: Option<u64>,
    /// Monotonic event timestamp in nanoseconds.
    pub monotonic_ns: u64,
    /// BLAKE3 of the complete normalized semantic state.
    pub state_hash: String,
    /// Whether this state was eligible for presentation.
    pub presented: bool,
    /// Ghostty's global dirty classification.
    pub ghostty_dirty: DirtyState,
    /// Rows Ghostty marked dirty.
    pub ghostty_dirty_rows: Vec<u16>,
    /// Stable delta from the preceding state of the same replay.
    pub delta: StateDelta,
}

/// Compact event/video mapping information for one state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateIndexEntry {
    /// State stream position.
    pub state_id: u64,
    /// State hash.
    pub state_hash: String,
    /// Source event.
    pub event_seq: Option<u64>,
    /// Source event byte offset.
    pub event_byte_offset: Option<u64>,
    /// Monotonic state time.
    pub monotonic_ns: u64,
    /// Whether a native renderer could present this state.
    pub presented: bool,
    /// Whether the checkpoint catalog contains an engine checkpoint here.
    pub checkpoint: bool,
}

/// Seek position for a canonical event's first control-index entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSourceIndexEntry {
    /// Canonical event sequence.
    pub event_seq: u64,
    /// Byte offset of the corresponding control-index entry.
    pub control_index_offset: u64,
}

/// Catalog entry for one engine checkpoint.
///
/// The frame holds the checkpointed state's record and an exact libghostty-vt engine snapshot,
/// so any later position can be rebuilt by restoring it and replaying up to that position.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticReplayCheckpoint {
    /// Stream position of the checkpointed state.
    pub state_id: u64,
    /// Canonical event responsible for the checkpointed state.
    pub event_seq: Option<u64>,
    /// Inclusive output byte offset of the checkpointed state inside its event.
    pub event_byte_offset: Option<u64>,
    /// Monotonic timestamp of the checkpointed state in nanoseconds.
    pub monotonic_ns: u64,
    /// Whether the checkpointed state was eligible for presentation.
    pub presented: bool,
    /// Number of boundary states before the checkpointed state.
    pub state_rank: u64,
    /// Number of hidden boundary states before the checkpointed state.
    pub hidden_before: u64,
    /// Combined semantic/effect/canonical/control timeline rank of the checkpointed state.
    pub timeline_rank: u64,
    /// Byte offset of the first control-index entry at or after the checkpointed state.
    pub control_index_offset: u64,
    /// Number of control entries ordered before the checkpointed state.
    pub control_rank: u64,
    /// Event-log position of the checkpointed state's event, where replay can resume.
    pub source_position: Option<EventLogPosition>,
    /// Byte offset of the checkpoint's zstd frame in the binary checkpoint member.
    pub offset: u64,
    /// Compressed byte length of that frame.
    pub length: u64,
}

/// Summary of one deterministic replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayArtifacts {
    /// Stream position of the state at the selected interval's end.
    pub final_state_id: u64,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
    /// Ghostty-formatted final plain text.
    pub final_text: Vec<u8>,
    /// Ghostty-formatted final VT reconstruction.
    pub final_vt: Vec<u8>,
    /// Ghostty-formatted final HTML.
    pub final_html: Vec<u8>,
}

/// In-memory replay result intended for tests and programmatic inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectedReplay {
    /// Every boundary state.
    pub states: Vec<StateRecord>,
    /// Final derived outputs.
    pub artifacts: ReplayArtifacts,
}

/// Selector for one reconstructed semantic state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateSelector {
    /// State at the end of the log.
    Final,
    /// Latest state eligible for terminal presentation.
    LatestPresented,
    /// State at an exact stream position.
    StateId(u64),
    /// Latest position in the run of equal states that holds the last boundary state with this
    /// hash. Finding the run end replays each position up to the next boundary; a hash that no
    /// boundary state shows replays every position of the log.
    StateHash(String),
    /// Latest state at or before an event sequence.
    EventSeq(u64),
    /// Latest state at or before a monotonic timestamp.
    MonotonicNs(u64),
}

/// One fully reconstructed state and its index metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateInspection {
    /// State index metadata and delta provenance.
    pub record: StateRecord,
    /// Complete semantic state, not a delta.
    pub state: SemanticState,
}

/// Manifest committed with derived replay artifacts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayManifest {
    /// Artifact schema version.
    pub version: u32,
    /// Source session identifier.
    pub session_id: Uuid,
    /// Source event-log terminal hash.
    pub source_event_hash: Option<String>,
    /// Selected half-open source interval, absent for a full-session index.
    pub source_range: Option<SourceRange>,
    /// Whether completion proved the source event log immutable.
    pub source_complete: bool,
    /// Semantic states listed in the state index.
    pub state_selection: StateSelection,
    /// Stride whose last presented state alone is listed, absent when every selected state is.
    pub sampling_stride_ns: Option<u64>,
    /// Exact Ghostty source revision.
    pub ghostty_revision: String,
    /// Number of entries in the state index.
    pub state_count: u64,
    /// Stream position at the selected interval's end.
    pub final_state_id: u64,
    /// Stream position of the latest state eligible for presentation, absent when none was.
    pub latest_presented_state_id: Option<u64>,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
    /// Hash of the final state retained in filtered artifact files.
    pub final_retained_state_hash: Option<String>,
    /// Whole-file integrity and classification for every payload member.
    pub members: Vec<ArtifactMember>,
}

/// Completed indexing paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexOutcome {
    /// Atomically committed derived directory.
    pub directory: PathBuf,
    /// Number of entries in the state index.
    pub state_count: u64,
    /// Stream position at the selected interval's end.
    pub final_state_id: u64,
    /// Whether the source had immutable completion evidence.
    pub source_complete: bool,
    /// State retention policy used by this index.
    pub state_selection: StateSelection,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
}
