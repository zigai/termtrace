//! Deterministic byte-granular replay and semantic state indexing.

use std::path::PathBuf;

pub use termtrace_model::terminal::replay::{
    CollectedReplay, DERIVED_DIRECTORY, GHOSTTY_REVISION, IndexOptions, IndexOutcome,
    ReplayArtifacts, ReplayManifest, RowDiff, STATE_FORMAT_VERSION, SemanticReplayCheckpoint,
    SourceRange, StateDelta, StateIndexEntry, StateInspection, StateRecord, StateSelection,
    StateSelector,
};
use termtrace_model::{execution::event::Event, terminal::state::SemanticState};

use crate::Error;

mod delta;
mod engine;
mod index;

use delta::state_delta;
#[cfg(test)]
use engine::{
    DEFAULT_REPLAY_LIMITS, ReplayLimits, replay_with, replay_with_limits,
    replay_with_seed_and_limits,
};
pub use engine::{
    RenderedStateFormats, StateInspectionWithFormats, format_state, inspect, inspect_many,
    inspect_with_formats, replay, semantic_state_hash, visit_states,
};
pub(crate) use engine::{
    ReplaySeed, inspect_latest_presented_fast, visit_states_range, visit_states_range_while,
    visit_states_seeded_while,
};
use engine::{replay_for_index, validate_replay_seed};
#[cfg(test)]
pub(crate) use engine::{reset_test_replay_output_bytes, test_replay_output_bytes};
pub(crate) use index::load_nearest_replay_seed;
#[cfg(test)]
pub(crate) use index::write_index;
pub use index::{
    derived_directory_name, derived_directory_name_for, index_session, index_session_range,
    index_session_with_options,
};

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
