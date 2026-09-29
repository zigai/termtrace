//! Deterministic byte-granular replay and semantic state indexing.

use crate::Error;
use std::path::PathBuf;
pub use termtrace_model::terminal::replay::{
    CollectedReplay, IndexOptions, IndexOutcome, ReplayArtifacts, ReplayManifest, RowDiff,
    SemanticReplayCheckpoint, SourceRange, StateDelta, StateIndexEntry, StateInspection,
    StateRecord, StateSelection, StateSelector, GHOSTTY_REVISION, STATE_FORMAT_VERSION,
};
use termtrace_model::{execution::event::Event, terminal::state::SemanticState};

mod delta;
mod engine;
mod index;
pub(crate) mod trust;

use delta::state_delta;
use engine::replay_for_index;
#[cfg(test)]
use engine::validate_replay_seed;
pub use engine::{
    format_state, inspect, inspect_completed_many, inspect_completed_session,
    inspect_completed_session_with_formats, inspect_many, inspect_with_formats, replay,
    semantic_state_hash, visit_session_states_range_while, visit_session_states_while,
    visit_snapshot_states_while, visit_states, RenderedStateFormats, StateInspectionWithFormats,
};
pub(crate) use engine::{
    inspect_latest_presented_prefix, inspect_latest_presented_stream, inspect_prefix_stream,
    inspect_session_stream, inspect_session_stream_with_formats,
    visit_recording_states_range_while, visit_states_range, visit_states_range_while,
    visit_states_seeded_while, ReplaySeed,
};
#[cfg(test)]
use engine::{
    replay_with, replay_with_limits, replay_with_seed_and_limits, ReplayLimits,
    DEFAULT_REPLAY_LIMITS,
};
#[cfg(test)]
pub(crate) use engine::{reset_test_replay_output_bytes, test_replay_output_bytes};
#[cfg(test)]
pub(crate) use index::write_cataloged_index;
pub use index::{
    derived_directory_name, derived_directory_name_for, index_session, index_session_for_video,
    index_session_range, index_session_with_options, index_session_with_stride,
};
pub(crate) use index::{DerivedIndex, CONTROL_INDEX, CONTROL_SOURCE_INDEX};
pub use trust::ValidatedSession;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EndpointInspection {
    pub state: SemanticState,
    pub state_hash: String,
    pub event_seq: Option<u64>,
    pub event_byte_offset: Option<u64>,
    pub monotonic_ns: Option<u64>,
    pub presented: bool,
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn invalid_replay(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
        source: None,
    };
}

pub(crate) fn timeline_event(event: &Event) -> bool {
    return !matches!(
        event,
        Event::Output { .. }
            | Event::InputResult { .. }
            | Event::ResizeResult { .. }
            | Event::SignalResult { .. }
    );
}

#[cfg(test)]
mod tests;
