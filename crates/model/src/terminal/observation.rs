//! Stable contracts for truth-labeled terminal observations.

use crate::{
    execution::event_log::CanonicalBoundary,
    terminal::replay::StateSelector,
    terminal::state::{ModeState, SemanticState},
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Durable and latest actor-processed positions returned by live operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorBoundaries {
    /// Latest boundary whose file synchronization completed successfully.
    pub durable: CanonicalBoundary,
    /// Latest boundary processed by the session actor, possibly volatile.
    pub observed: CanonicalBoundary,
}

/// Disposable full-state checkpoint bound to exact canonical source bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCheckpoint {
    /// Checkpoint format version.
    pub version: u32,
    /// Pinned semantic state format.
    pub state_format_version: u32,
    /// Pinned semantic engine revision.
    pub ghostty_revision: String,
    /// Exact requested state point.
    pub state_point: StatePoint,
    /// BLAKE3 of exact completion JSON bytes.
    pub completion_blake3: String,
    /// Completion-bound final canonical event hash.
    pub final_event_hash: String,
    /// Dense canonical state identifier when byte-granular replay established it.
    pub state_id: Option<u64>,
    /// Canonical event responsible for the state, when known.
    pub event_seq: Option<u64>,
    /// Byte offset inside the source output event, when known.
    pub event_byte_offset: Option<u64>,
    /// Monotonic source timestamp, when known.
    pub monotonic_ns: Option<u64>,
    /// BLAKE3 semantic-state hash.
    pub state_hash: String,
    /// Whether the state was eligible for terminal presentation.
    pub presented: bool,
    /// Complete semantic state.
    pub state: SemanticState,
    /// BLAKE3 of canonical JSON for the complete cached semantic state.
    pub state_blake3: String,
}

/// Bounded disposable replay-cache inventory for one canonical session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCacheStatus {
    /// Cache directory inside the canonical session.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub directory: PathBuf,
    /// Number of checkpoint files currently retained.
    pub entries: u64,
    /// Total compressed bytes retained.
    pub compressed_bytes: u64,
    /// Automatic per-session checkpoint limit.
    pub maximum_entries: u64,
    /// Automatic per-session compressed-byte limit.
    pub maximum_compressed_bytes: u64,
}

/// Result of explicitly deleting disposable replay checkpoints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCachePruneReport {
    /// Cache directory considered.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub directory: PathBuf,
    /// Files removed.
    pub removed_entries: u64,
    /// Compressed bytes removed.
    pub removed_bytes: u64,
    /// Inventory remaining after pruning.
    pub remaining: ReplayCacheStatus,
}

/// Source category for one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSourceKind {
    /// Responsive canonical recording actor.
    LiveSession,
    /// Validated completed canonical recording.
    CompletedSession,
    /// Explicitly validated but incomplete canonical event prefix.
    InterruptedSession,
    /// Unchained ANSI/VT byte stream.
    AnsiStream,
    /// Completed canonical evidence imported from an ANSI/VT byte stream.
    ImportedAnsi,
    /// Canonical recording allocated for a one-off command.
    OneOffCommand,
}

/// How settled the observed state is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureQuality {
    /// Immediate state from a responsive live actor.
    Live,
    /// Live state proven unchanged for a requested interval.
    Stable,
    /// State from validated completed evidence.
    Completed,
    /// Explicitly incomplete validated prefix.
    Incomplete,
}

/// Rendering truth represented by an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TruthLayer {
    /// Pinned semantic-engine state.
    Semantic,
    /// Pixels captured from the native terminal renderer.
    Native,
}

/// Integrity relationship between an observation and canonical evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationIntegrity {
    /// State derives from a validated canonical hash chain or live actor boundary.
    CanonicalChained,
    /// State derives from an unchained byte stream.
    Unchained,
}

/// Service path used to resolve an observation without changing its meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationResolution {
    /// Actor-consistent live snapshot fast path.
    ActorSnapshot,
    /// Independently validated canonical event replay.
    CanonicalReplay,
    /// Content-addressed full-state checkpoint validated against exact source bytes.
    ValidatedCheckpoint,
    /// Pinned semantic interpretation of an unchained ANSI stream.
    AnsiInterpretation,
}

/// Shared state selection vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "selector", rename_all = "snake_case", deny_unknown_fields)]
pub enum StatePoint {
    /// Latest presentation-eligible state.
    LatestPresented,
    /// Exact semantic state identifier.
    StateId {
        /// Exact semantic state identifier.
        state_id: u64,
    },
    /// Exact content-addressed semantic state hash.
    StateHash {
        /// BLAKE3 hash of the complete normalized semantic state.
        state_hash: String,
    },
    /// Latest state at or before an event sequence.
    EventSeq {
        /// Canonical event sequence.
        event_seq: u64,
    },
    /// Latest state at or before a session-monotonic timestamp.
    MonotonicNs {
        /// Session-monotonic nanoseconds.
        monotonic_ns: u64,
    },
    /// State at a uniquely named marker.
    Marker {
        /// Unique marker label.
        label: String,
    },
}

impl StatePoint {
    /// Translate to a replay selector, resolving a marker label to its event sequence.
    ///
    /// # Errors
    ///
    /// Returns the resolver's error for a missing or ambiguous marker.
    pub fn selector<E>(
        &self,
        marker_event_seq: impl FnOnce(&str) -> Result<u64, E>,
    ) -> Result<StateSelector, E> {
        return Ok(match self {
            Self::LatestPresented => StateSelector::LatestPresented,
            Self::StateId { state_id } => StateSelector::StateId(*state_id),
            Self::StateHash { state_hash } => StateSelector::StateHash(state_hash.clone()),
            Self::EventSeq { event_seq } => StateSelector::EventSeq(*event_seq),
            Self::MonotonicNs { monotonic_ns } => StateSelector::MonotonicNs(*monotonic_ns),
            Self::Marker { label } => StateSelector::EventSeq(marker_event_seq(label)?),
        });
    }
}

/// Compact stable frame shared by CLI and clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationFrame {
    /// Plain text for the complete viewport.
    pub viewport_text: String,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Cursor column when it lies in the viewport.
    pub cursor_column: Option<u16>,
    /// Cursor row when it lies in the viewport.
    pub cursor_row: Option<u16>,
    /// Whether the cursor is visible.
    pub cursor_visible: bool,
    /// Stable terminal-mode facts.
    pub modes: ModeState,
}

/// Compact versioned frame JSON optimized for test clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameEnvelope {
    /// Stable frame format version.
    pub version: u32,
    /// Semantic state hash linked to the full observation.
    pub state_hash: String,
    /// Whether this state was eligible for terminal presentation.
    pub presented: bool,
    /// Settlement quality of the source observation.
    pub capture_quality: CaptureQuality,
    /// Compact frame payload.
    pub frame: ObservationFrame,
    /// BLAKE3 of canonical JSON for `frame`.
    pub frame_hash: String,
}

/// Truth-labeled observation of one terminal state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Source category.
    pub source_kind: ObservationSourceKind,
    /// Canonical session ID when evidence exists.
    pub session_id: Option<Uuid>,
    /// Exact canonical session path bytes when evidence exists.
    pub canonical_session: Option<ByteString>,
    /// Whether canonical execution evidence was durably completed.
    pub source_completed: bool,
    /// Requested state point.
    pub state_point: StatePoint,
    /// Semantic state ID when replay assigned one.
    pub state_id: Option<u64>,
    /// BLAKE3 of the complete semantic state.
    pub state_hash: String,
    /// Responsible canonical event sequence when known.
    pub event_seq: Option<u64>,
    /// Session-monotonic timestamp when known.
    pub monotonic_ns: Option<u64>,
    /// Whether this state was presentation eligible.
    pub presented: bool,
    /// Source boundary observed or validated.
    pub boundary: CanonicalBoundary,
    /// Settlement quality.
    pub capture_quality: CaptureQuality,
    /// Rendering truth layer.
    pub truth_layer: TruthLayer,
    /// Integrity relationship to canonical execution evidence.
    pub integrity: ObservationIntegrity,
    /// Service path used to resolve this state.
    pub resolution: ObservationResolution,
    /// Compact stable frame.
    pub frame: ObservationFrame,
    /// BLAKE3 of canonical JSON for `frame`.
    pub payload_hash: String,
}

impl Observation {
    /// Return the compact stable frame contract linked to this observation.
    #[must_use]
    pub fn frame_envelope(&self) -> FrameEnvelope {
        return FrameEnvelope {
            version: 1,
            state_hash: self.state_hash.clone(),
            presented: self.presented,
            capture_quality: self.capture_quality,
            frame: self.frame.clone(),
            frame_hash: self.payload_hash.clone(),
        };
    }
}
