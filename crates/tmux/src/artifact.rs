//! Versioned on-disk and export contracts for aggregate tmux recordings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};
use termtrace_model::value::ByteString;
use uuid::Uuid;

/// Stable aggregate tmux recording manifest format.
pub const TMUX_RECORDING_FORMAT: &str = "termtrace-tmux-recording";
/// Current aggregate tmux recording manifest version.
pub const TMUX_RECORDING_VERSION: u32 = 5;
/// Current lifecycle journal record shape, bound by every committed boundary.
pub const TMUX_JOURNAL_VERSION: u32 = 2;

/// Coverage policy committed by an aggregate tmux recording.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxRecordingCoverage {
    /// Report uncovered panes without terminating them.
    Warn,
    /// Permit termination of uncovered panes after the configured grace period.
    Strict,
}

impl TmuxRecordingCoverage {
    /// Stable option and wire spelling.
    pub fn as_str(self) -> &'static str {
        return match self {
            Self::Warn => "warn",
            Self::Strict => "strict",
        };
    }
}

impl FromStr for TmuxRecordingCoverage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        return match value {
            "warn" => Ok(Self::Warn),
            "strict" => Ok(Self::Strict),
            _ => Err(format!("unsupported tmux coverage mode `{value}`")),
        };
    }
}

/// Aggregate recording lifecycle; see [`crate::lifecycle`] for its legal transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxRecordingLifecycle {
    /// Aggregate identity exists while its composite client is starting.
    Starting,
    /// Composite and pane-track supervision are active.
    Live,
    /// A stop request has been committed.
    Stopping,
    /// The composite and every pane track completed; history is sealed.
    Completed,
}

/// How a tmux client reaches its server.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxSocket {
    /// tmux's default socket selection.
    #[default]
    Default,
    /// A socket name, equivalent to `tmux -L`.
    Name(ByteString),
    /// A socket path, equivalent to `tmux -S`.
    Path(ByteString),
}

/// Immutable identity and exact path of one canonical Termtrace session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxSessionLink {
    /// Canonical Termtrace session identity.
    pub session_id: Uuid,
    /// Exact canonical session path bytes.
    pub session: ByteString,
}

/// Link from an aggregate recording to its outer tmux client recording.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxCompositeTrack {
    /// Registry alias for the outer-client track.
    pub alias: String,
    /// Canonical session once the composite client started.
    pub link: Option<TmuxSessionLink>,
}

/// Immutable tmux server identity bound by the first successful observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxServerIdentity {
    /// tmux server PID.
    pub pid: u32,
    /// Server socket path bytes.
    pub socket_path: ByteString,
    /// Device containing the server socket.
    pub socket_device: u64,
    /// Inode of the server socket.
    pub socket_inode: u64,
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

impl TmuxPaneSnapshot {
    /// Stable object identifiers of this pane.
    pub fn address(&self) -> TmuxPaneAddress {
        return TmuxPaneAddress {
            session_id: self.session_id.clone(),
            window_id: self.window_id.clone(),
            pane_id: self.pane_id.clone(),
        };
    }
}

/// Stable tmux object identifiers of one pane at observation time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxPaneAddress {
    /// Stable tmux session ID.
    pub session_id: String,
    /// Stable tmux window ID.
    pub window_id: String,
    /// Stable tmux pane ID.
    pub pane_id: String,
}

/// Category of an uncovered pane or observation limitation.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TmuxGapKind {
    /// Transitions coalesced between supervisor observations, or made without a tmux hook,
    /// may only be visible in the composite client track.
    PollingInterval,
    /// tmux messages and command prompts are captured by the composite client track but are
    /// not separately indexed in lifecycle events.
    ClientOverlayIndex,
    /// Aggregate observation resumed; transitions since the preceding snapshot may be missing.
    /// Pane recording runs independently.
    ObserverStart,
    /// A selected pane's root process is not a Termtrace pane wrapper.
    UncoveredPane,
}

impl TmuxGapKind {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        return match self {
            Self::PollingInterval => "polling_interval",
            Self::ClientOverlayIndex => "client_overlay_index",
            Self::ObserverStart => "observer_start",
            Self::UncoveredPane => "uncovered_pane",
        };
    }
}

/// One uncovered pane or explicit observation limitation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxCoverageGap {
    /// Related pane ID, or none for an aggregate limitation.
    pub pane_id: Option<String>,
    /// Gap category.
    pub kind: TmuxGapKind,
    /// First observation wall-clock time.
    pub first_observed_unix_ns: u64,
    /// Most recent observation wall-clock time.
    pub last_observed_unix_ns: u64,
    /// Wall-clock time when the gap was first observed to be resolved.
    pub resolved_unix_ns: Option<u64>,
}

impl TmuxCoverageGap {
    /// A newly observed, unresolved gap.
    pub fn open(pane_id: Option<String>, kind: TmuxGapKind, now: u64) -> Self {
        return Self {
            pane_id,
            kind,
            first_observed_unix_ns: now,
            last_observed_unix_ns: now,
            resolved_unix_ns: None,
        };
    }

    /// Whether both values describe the same interval subject.
    pub fn same_subject(&self, other: &Self) -> bool {
        return self.pane_id == other.pane_id && self.kind == other.kind;
    }
}

/// One immutable pane recording incarnation linked to an aggregate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxPaneTrack {
    /// Stable pane ID; a pane may have arbitrarily many recording incarnations.
    pub pane_id: String,
    /// Immutable canonical recording identity.
    pub session_id: Uuid,
    /// Exact canonical session path.
    pub session: ByteString,
    /// Time this incarnation was first linked to the aggregate.
    pub linked_unix_ns: u64,
}

/// One typed aggregate lifecycle event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TmuxEvent {
    /// The aggregate identity was created.
    RecordingStarted {
        /// Committed coverage policy.
        coverage: TmuxRecordingCoverage,
    },
    /// The composite client track started.
    CompositeTrackLinked {
        /// Composite session identity and path.
        link: TmuxSessionLink,
    },
    /// A stop request was committed.
    RecordingStopping,
    /// Every track completed; no further events follow.
    RecordingCompleted,
    /// The first reachable server identity was bound.
    ServerIdentityBound {
        /// Bound identity.
        identity: TmuxServerIdentity,
    },
    /// A supervisor process started observing the server.
    ObserverStarted {
        /// Supervisor process ID.
        pid: u32,
    },
    /// A tmux session appeared.
    SessionCreated {
        /// Stable tmux session ID.
        session_id: String,
    },
    /// A tmux session disappeared.
    SessionDestroyed {
        /// Stable tmux session ID.
        session_id: String,
    },
    /// A tmux window appeared.
    WindowCreated {
        /// Stable tmux window ID.
        window_id: String,
    },
    /// A tmux window disappeared.
    WindowDestroyed {
        /// Stable tmux window ID.
        window_id: String,
    },
    /// A pane appeared.
    PaneCreated {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
    },
    /// A pane disappeared.
    PaneDestroyed {
        /// Last observed pane identifiers.
        pane: TmuxPaneAddress,
    },
    /// A pane moved into another window.
    PaneMoved {
        /// Pane identifiers after the move.
        pane: TmuxPaneAddress,
    },
    /// A pane's window layout changed.
    LayoutChanged {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
        /// New layout.
        layout: String,
    },
    /// A pane's window became active or inactive.
    ActiveWindowChanged {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
        /// Whether the window is now active.
        active: bool,
    },
    /// A pane became active or inactive in its window.
    ActivePaneChanged {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
        /// Whether the pane is now active.
        active: bool,
    },
    /// A pane's window was zoomed or unzoomed.
    ZoomChanged {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
        /// Whether the window is now zoomed.
        zoomed: bool,
    },
    /// A pane entered, changed, or left a tmux mode such as copy mode.
    PaneModeChanged {
        /// Pane identifiers.
        pane: TmuxPaneAddress,
        /// Active mode name, or none when no mode is active.
        mode: Option<String>,
    },
    /// A pane recording incarnation was linked.
    PaneTrackLinked {
        /// Linked incarnation.
        track: TmuxPaneTrack,
    },
    /// A linked pane recording published canonical completion.
    PaneTrackCompleted {
        /// Stable pane ID.
        pane_id: String,
        /// Completed recording identity.
        session_id: Uuid,
    },
    /// A coverage interval opened.
    CoverageGapOpened {
        /// Opened interval.
        gap: TmuxCoverageGap,
    },
    /// A coverage interval resolved.
    CoverageGapResolved {
        /// Final interval state.
        gap: TmuxCoverageGap,
    },
}

/// One sequenced, timestamped lifecycle event as committed to the journal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxEventRecord {
    /// Monotonic sequence within this aggregate, starting at one.
    pub sequence: u64,
    /// Observation wall-clock time.
    pub observed_unix_ns: u64,
    /// Typed event.
    #[serde(flatten)]
    pub event: TmuxEvent,
}

/// Committed prefix of the append-only lifecycle journal beside the snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxJournalBoundary {
    /// Journal record shape version.
    pub version: u32,
    /// Complete newline-framed bytes committed by this snapshot.
    pub bytes: u64,
    /// Last committed lifecycle sequence.
    pub sequence: u64,
    /// Hash of the last journal record.
    pub hash: String,
}

impl TmuxJournalBoundary {
    /// Boundary of a journal with no committed records.
    pub fn empty() -> Self {
        return Self {
            version: TMUX_JOURNAL_VERSION,
            bytes: 0,
            sequence: 0,
            hash: String::new(),
        };
    }
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxGapCatalogEntry {
    /// Lifecycle sequence that opened this interval.
    pub sequence: u64,
    /// Latest committed state of the interval.
    pub gap: TmuxCoverageGap,
}

/// Current aggregate state, changed only by [`crate::reducer::reduce`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TmuxRecordingState {
    /// Aggregate lifecycle.
    pub lifecycle: TmuxRecordingLifecycle,
    /// Link to the composite outer-client recording.
    pub composite_track: TmuxCompositeTrack,
    /// Immutable tmux server identity once observed.
    pub server: Option<TmuxServerIdentity>,
    /// Latest per-pane observations.
    pub panes: Vec<TmuxPaneSnapshot>,
    /// Present pane IDs linked to their current pane-specific session path.
    pub pane_tracks: BTreeMap<String, ByteString>,
    /// Unresolved coverage gaps; resolved intervals live in the journal and catalog.
    pub coverage_gaps: Vec<TmuxCoverageGap>,
    /// Last state change wall-clock time.
    pub updated_unix_ns: u64,
}

/// Aggregate identity and current state; complete history lives in the lifecycle journal.
///
/// The snapshot holds no history: events, resolved gaps, and every track incarnation are
/// read through the journal-backed catalog.
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
    /// Creation wall-clock time.
    pub created_unix_ns: u64,
    /// Coverage enforcement policy.
    pub coverage: TmuxRecordingCoverage,
    /// Exact tmux executable path bytes.
    pub tmux_executable: ByteString,
    /// Configured tmux server socket.
    pub socket: TmuxSocket,
    /// Current aggregate state.
    #[serde(flatten)]
    pub state: TmuxRecordingState,
    /// Committed lifecycle journal prefix.
    pub lifecycle_journal: TmuxJournalBoundary,
    /// Complete historical counts indexed from the journal.
    pub catalog: TmuxCatalogSummary,
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
    /// BLAKE3 of tracks.jsonl; each row is a `TmuxBundleTrack`.
    pub track_inventory_blake3: String,
}
