//! Stable, truth-labeled terminal observation envelopes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{event::ByteString, event_log::CanonicalBoundary, ghostty_vt::ModeState};

/// Source category for one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSourceKind {
    /// Responsive canonical recording actor.
    LiveSession,
    /// Validated completed canonical recording.
    CompletedSession,
    /// Unchained ANSI/VT byte stream.
    AnsiStream,
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
    /// Compact stable frame.
    pub frame: ObservationFrame,
    /// BLAKE3 of canonical JSON for `frame`.
    pub payload_hash: String,
}
