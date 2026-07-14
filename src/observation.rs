//! Stable, truth-labeled terminal observation envelopes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use std::{
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

use crate::{
    Error, Result,
    control::{ControlCommand, ControlData, request},
    event::{ByteString, Event},
    event_log::{CanonicalBoundary, EventLog},
    ghostty_vt::{ModeState, SemanticState},
    replay::{StateSelector, inspect},
    session::{read_manifest, validate_completed_log},
};

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

/// Observe a responsive live session or validated completed session.
///
/// # Errors
///
/// Returns an error when canonical identity/validation fails, a live selector
/// is not supported, actor observation fails, or replay cannot resolve the point.
pub fn observe_session(session: &Path, state_point: StatePoint) -> Result<Observation> {
    return observe_session_with_options(session, state_point, false);
}

/// Observe a session with explicit permission to select hidden semantic states.
///
/// # Errors
///
/// Returns the same errors as [`observe_session`] and rejects a hidden selected
/// state unless `include_hidden` is true.
pub fn observe_session_with_options(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let manifest = read_manifest(&canonical)?;
    if canonical.join("completion.json").is_file() {
        return observe_completed(&canonical, manifest.session_id, state_point, include_hidden);
    }
    if state_point != StatePoint::LatestPresented {
        return Err(observation_error(
            "live sessions currently support only latest-presented selection",
        ));
    }
    let snapshot = request(&canonical, ControlCommand::Snapshot)?;
    let ControlData::Snapshot {
        viewport_text,
        state_hash,
        presented,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
        observed_monotonic_ns,
        boundaries,
    } = snapshot
    else {
        return Err(observation_error(
            "actor returned an unexpected observation",
        ));
    };
    let frame = ObservationFrame {
        viewport_text,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
    };
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::LiveSession,
        session_id: Some(manifest.session_id),
        canonical_session: Some(ByteString::from(canonical.as_os_str().as_bytes())),
        source_completed: false,
        state_point,
        state_id: None,
        state_hash,
        event_seq: boundaries.observed.next_event_seq.checked_sub(1),
        monotonic_ns: Some(observed_monotonic_ns),
        presented,
        boundary: boundaries.observed,
        capture_quality: CaptureQuality::Live,
        truth_layer: TruthLayer::Semantic,
        frame,
        payload_hash,
    });
}

fn observe_completed(
    session: &Path,
    session_id: Uuid,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let log = EventLog::read(session.join("events.jsonl"))?;
    let completion = validate_completed_log(session, &log)?;
    let selector = match &point {
        StatePoint::LatestPresented => StateSelector::LatestPresented,
        StatePoint::StateId { state_id } => StateSelector::StateId(*state_id),
        StatePoint::EventSeq { event_seq } => StateSelector::EventSeq(*event_seq),
        StatePoint::MonotonicNs { monotonic_ns } => StateSelector::MonotonicNs(*monotonic_ns),
        StatePoint::Marker { label } => {
            let mut matches = log.events.iter().filter(|record| {
                return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
            });
            let event_seq = matches
                .next()
                .map(|record| return record.seq)
                .ok_or_else(|| return observation_error("marker does not exist"))?;
            if matches.next().is_some() {
                return Err(observation_error("marker label is ambiguous"));
            }
            StateSelector::EventSeq(event_seq)
        }
    };
    let inspection = inspect(&log, selector)?;
    if !inspection.record.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash,
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash,
            output_bytes: completion.output_bytes,
        },
        capture_quality: CaptureQuality::Completed,
        truth_layer: TruthLayer::Semantic,
        frame,
        payload_hash,
    });
}

fn frame_from_state(state: &SemanticState) -> ObservationFrame {
    return ObservationFrame {
        viewport_text: viewport_text(state),
        cols: state.cols,
        rows: state.rows,
        cursor_column: state.cursor.column,
        cursor_row: state.cursor.row,
        cursor_visible: state.cursor.visible,
        modes: state.modes.clone(),
    };
}

fn viewport_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        let start = output.len();
        for cell in &row.cells {
            if cell.grapheme.is_empty() {
                output.push(' ');
            } else {
                output.push_str(&cell.grapheme);
            }
        }
        while output.len() > start && output.ends_with(' ') {
            output.pop();
        }
        if index + 1 < state.grid.len() {
            output.push('\n');
        }
    }
    return output;
}

fn frame_hash(frame: &ObservationFrame) -> Result<String> {
    let bytes = serde_json::to_vec(frame).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<observation-frame>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

fn observation_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "observe terminal state",
        reason: reason.into(),
    };
}
