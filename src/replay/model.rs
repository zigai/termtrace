use super::*;

use crate::event::ByteString;

/// Pinned Ghostty source revision used by the semantic adapter.
pub const GHOSTTY_REVISION: &str = "55a3e33ab26a23d75b274b23c7f76d837db00578";
/// Derived artifact format version.
pub const STATE_FORMAT_VERSION: u32 = 4;
/// Relative directory containing replay-derived artifacts.
pub const DERIVED_DIRECTORY: &str = "derived";

/// Semantic states retained in one derived index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateSelection {
    /// Retain every distinct state, including synchronized-output internals.
    #[default]
    All,
    /// Retain only states eligible for presentation by a native terminal.
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
}

/// Fine-grained changes for one viewport row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RowDiff {
    /// Zero-based row.
    pub row: u16,
    /// Cells whose complete semantic value changed.
    pub cells: Vec<CellState>,
}

/// Delta from the previous distinct state.
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
    /// Cell-level row deltas.
    pub rows: Vec<RowDiff>,
}

/// One distinct semantic state boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateRecord {
    /// Dense zero-based state identifier.
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
    /// Stable delta from the preceding distinct state.
    pub delta: StateDelta,
}

/// Compact event/video mapping information for one state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateIndexEntry {
    /// State identifier.
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
    /// Whether the checkpoint stream contains a full-state replay seed here.
    #[serde(default)]
    pub checkpoint: bool,
    /// Combined semantic/canonical/control timeline rank for this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_rank: Option<u64>,
    /// Byte offset of the first control-index entry at or after this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_index_offset: Option<u64>,
    /// Number of control entries ordered before this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_rank: Option<u64>,
}

/// Periodic full-state replay seed addressable from the compact state index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticReplayCheckpoint {
    /// Canonical state boundary represented by this seed.
    pub inspection: StateInspection,
    /// Ghostty VT reconstruction validated against the semantic state hash before use.
    pub engine_vt: ByteString,
}

/// Summary of one deterministic replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayArtifacts {
    /// Number of distinct states, including the initial blank state.
    pub state_count: u64,
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
    /// Every distinct state delta.
    pub states: Vec<StateRecord>,
    /// Final derived outputs.
    pub artifacts: ReplayArtifacts,
}

/// Selector for one reconstructed semantic state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateSelector {
    /// Final distinct state.
    Final,
    /// Latest state eligible for terminal presentation.
    LatestPresented,
    /// Exact state identifier.
    StateId(u64),
    /// Exact content-addressed semantic state hash.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EndpointInspection {
    pub state: SemanticState,
    pub state_hash: String,
    pub event_seq: Option<u64>,
    pub event_byte_offset: Option<u64>,
    pub monotonic_ns: Option<u64>,
    pub presented: bool,
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
    /// Semantic states retained in the JSONL artifacts.
    pub state_selection: StateSelection,
    /// Exact Ghostty source revision.
    pub ghostty_revision: String,
    /// Number of state records retained in the artifact files.
    pub state_count: u64,
    /// Number of distinct semantic states encountered during replay.
    pub semantic_state_count: u64,
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
    /// Number of state records retained in artifacts.
    pub state_count: u64,
    /// Total distinct states encountered during replay.
    pub semantic_state_count: u64,
    /// Whether the source had immutable completion evidence.
    pub source_complete: bool,
    /// State retention policy used by this index.
    pub state_selection: StateSelection,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
}
