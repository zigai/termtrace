//! Aggregate identity, lifecycle, and bundle contracts for full tmux recordings.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{artifact::metadata::ArtifactMember, value::ByteString};

/// Stable aggregate tmux recording manifest format.
pub const TMUX_RECORDING_FORMAT: &str = "termtrace-tmux-recording";
/// Current aggregate tmux recording manifest version.
pub const TMUX_RECORDING_VERSION: u32 = 4;
/// Stable aggregate tmux bundle format.
pub const TMUX_BUNDLE_FORMAT: &str = "termtrace-tmux-bundle";
/// Current aggregate tmux bundle schema version.
pub const TMUX_BUNDLE_VERSION: u32 = 1;

/// Coverage policy committed by an aggregate tmux recording.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxRecordingCoverage {
    /// Report uncovered panes without terminating them.
    Warn,
    /// Permit termination of uncovered panes after the configured grace period.
    Strict,
}

/// Aggregate recording lifecycle.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxRecordingLifecycle {
    /// Aggregate identity exists while its composite client is starting.
    Starting,
    /// Composite and pane-track supervision are active.
    Live,
    /// A stop request has been committed.
    Stopping,
    /// The composite client recording completed.
    Completed,
}

/// Link from an aggregate recording to its outer tmux client recording.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxCompositeTrack {
    /// Registry alias for the outer-client track.
    pub alias: String,
    /// Canonical Termtrace session identity once linked.
    pub session_id: Option<Uuid>,
    /// Canonical Termtrace session path once linked.
    pub session: Option<ByteString>,
    /// Outer-client track lifecycle.
    pub lifecycle: TmuxRecordingLifecycle,
}

/// Latest supervisor observation for one tmux pane.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "tmux exposes window activity, zoom, pane activity, and copy mode as independent protocol flags"
)]
pub struct TmuxPaneSnapshot {
    /// Stable tmux session ID.
    pub session_id: String,
    /// Human-readable tmux session name.
    pub session_name: String,
    /// Stable tmux window ID.
    pub window_id: String,
    /// Current tmux window index.
    pub window_index: u32,
    /// Current tmux window layout.
    pub layout: String,
    /// Whether this pane belongs to the active window.
    pub window_active: bool,
    /// Whether the pane's window is zoomed.
    pub zoomed: bool,
    /// Stable tmux pane ID.
    pub pane_id: String,
    /// Whether this pane is active in its window.
    pub pane_active: bool,
    /// Whether the pane is in a tmux mode such as copy mode.
    pub copy_mode: bool,
    /// Mode name reported by tmux.
    pub pane_mode: String,
    /// Canonical pane-specific Termtrace session path when covered.
    pub recording_session: Option<ByteString>,
}

/// One lifecycle transition observed by the tmux supervisor.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxLifecycleEvent {
    /// Monotonic sequence within this aggregate manifest.
    pub sequence: u64,
    /// Observation wall-clock time.
    pub observed_unix_ns: u128,
    /// Stable lifecycle event kind.
    pub kind: String,
    /// Related stable tmux session ID.
    pub session_id: Option<String>,
    /// Related stable tmux window ID.
    pub window_id: Option<String>,
    /// Related stable tmux pane ID.
    pub pane_id: Option<String>,
    /// Event-specific state or linked path.
    pub value: Option<String>,
}

/// Committed prefix of the append-only lifecycle journal beside the snapshot.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxJournalBoundary {
    /// Complete newline-framed bytes committed by this snapshot.
    pub bytes: u64,
    /// Last committed lifecycle sequence.
    pub sequence: u64,
    /// Hash of the last journal record.
    pub hash: String,
}

/// One uncovered pane or explicit observation limitation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxCoverageGap {
    /// Related pane ID, or none for an aggregate limitation.
    pub pane_id: Option<String>,
    /// Stable gap category.
    pub kind: String,
    /// Human-readable limitation or uncovered-state description.
    pub detail: String,
    /// First observation wall-clock time.
    #[serde(default)]
    pub first_observed_unix_ns: u128,
    /// Most recent observation wall-clock time.
    #[serde(default)]
    pub last_observed_unix_ns: u128,
    /// Wall-clock time when the gap was first observed to be resolved.
    #[serde(default)]
    pub resolved_unix_ns: Option<u128>,
}

/// Aggregate identity, links, lifecycle, and coverage for a full tmux recording.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxPaneTrack {
    /// Stable pane ID; a pane may have arbitrarily many recording incarnations.
    pub pane_id: String,
    /// Immutable canonical recording identity.
    pub session_id: Uuid,
    /// Exact canonical session path.
    pub session: ByteString,
    /// Time this incarnation was linked to the aggregate.
    pub linked_unix_ns: u128,
}

/// Counts committed with the lifecycle journal's indexed inventory.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxCatalogSummary {
    /// Recording incarnations retained in complete history.
    pub tracks: u64,
    /// Linked incarnations without an observed canonical completion.
    pub unfinished_tracks: u64,
    /// All coverage intervals, including resolved intervals.
    pub coverage_gaps: u64,
}

/// One bounded-page result from the complete recording inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxTrackCatalogEntry {
    /// Lifecycle sequence that first linked this incarnation.
    pub sequence: u64,
    /// Immutable pane recording identity and path.
    pub track: TmuxPaneTrack,
    /// Whether canonical completion was observed.
    pub completed: bool,
}

/// One historical coverage interval from the lifecycle catalog.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxGapCatalogEntry {
    /// Lifecycle sequence that opened this interval.
    pub sequence: u64,
    /// Latest committed state of the interval.
    pub gap: TmuxCoverageGap,
}

/// Aggregate identity, links, lifecycle, and coverage for a full tmux recording.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TmuxRecordingManifest {
    /// Stable format identifier.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Immutable aggregate recording identity.
    pub recording_id: Uuid,
    /// Friendly aggregate recording name.
    pub name: String,
    /// Aggregate lifecycle.
    pub lifecycle: TmuxRecordingLifecycle,
    /// Creation wall-clock time.
    pub created_unix_ns: u128,
    /// Last supervisor update wall-clock time.
    pub updated_unix_ns: u128,
    /// Coverage enforcement policy.
    pub coverage: TmuxRecordingCoverage,
    /// Exact tmux executable path bytes.
    pub tmux_executable: ByteString,
    /// Configured tmux socket name bytes.
    pub socket_name: Option<ByteString>,
    /// Configured tmux socket path bytes.
    pub socket_path: Option<ByteString>,
    /// Immutable tmux server PID captured by the first successful snapshot.
    pub server_pid: Option<u32>,
    /// Immutable tmux server socket path captured by the first successful snapshot.
    pub server_socket_path: Option<ByteString>,
    /// Device containing the immutable tmux server socket.
    pub server_socket_device: Option<u64>,
    /// Inode of the immutable tmux server socket.
    pub server_socket_inode: Option<u64>,
    /// Link to the composite outer-client recording.
    pub composite_track: TmuxCompositeTrack,
    /// Latest per-pane state observations.
    pub panes: Vec<TmuxPaneSnapshot>,
    /// Stable pane IDs linked to pane-specific Termtrace sessions.
    pub pane_tracks: BTreeMap<String, ByteString>,
    /// Recent recording incarnations, bounded to 128; the catalog retains complete history.
    pub track_history: BTreeMap<Uuid, TmuxPaneTrack>,
    /// Complete historical counts; the lifecycle journal is authoritative for this index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<TmuxCatalogSummary>,
    /// Observed pane, window, session, layout, focus, zoom, and mode transitions.
    pub events: Vec<TmuxLifecycleEvent>,
    /// Durable full lifecycle history; `events` contains only its recent tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_journal: Option<TmuxJournalBoundary>,
    /// Currently unresolved coverage gaps; resolved intervals remain in the journal/catalog.
    pub coverage_gaps: Vec<TmuxCoverageGap>,
}

/// One complete canonical track embedded in an aggregate tmux bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxBundleTrack {
    /// `composite` or `pane`.
    pub role: String,
    /// Stable tmux pane ID for a pane track.
    pub pane_id: Option<String>,
    /// Immutable canonical Termtrace session identity.
    pub session_id: Uuid,
    /// Exact source session path bytes.
    pub canonical_session: ByteString,
    /// Relative directory containing a forensic Termtrace bundle.
    pub directory: String,
    /// Final canonical event hash bound by the nested forensic bundle.
    pub source_final_event_hash: String,
}

/// Whole-file inventory for a complete aggregate tmux evidence bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxBundleManifest {
    /// Stable format identifier.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Immutable aggregate recording identity.
    pub recording_id: Uuid,
    /// Friendly aggregate recording name.
    pub recording_name: String,
    /// Exact source aggregate manifest path bytes.
    pub aggregate_manifest: ByteString,
    /// BLAKE3 of the complete source aggregate manifest.
    pub aggregate_manifest_blake3: String,
    /// Composite track followed by pane tracks in stable pane-ID order.
    pub tracks: Vec<TmuxBundleTrack>,
    /// Whole-file inventory excluding this root manifest.
    pub members: Vec<ArtifactMember>,
}

/// Bounded manifest for an aggregate export with a streamed track inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxStreamingBundleManifest {
    /// Stable format identifier.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Immutable aggregate recording identity.
    pub recording_id: Uuid,
    /// Friendly recording name.
    pub recording_name: String,
    /// Exact source aggregate manifest path bytes.
    pub aggregate_manifest: ByteString,
    /// BLAKE3 of the caller-owned aggregate snapshot.
    pub aggregate_manifest_blake3: String,
    /// Number of nested forensic bundles, including the composite.
    pub track_count: u64,
    /// Exact byte count of tracks.jsonl.
    pub track_inventory_bytes: u64,
    /// BLAKE3 of tracks.jsonl; each row is a TmuxBundleTrack.
    pub track_inventory_blake3: String,
}
