//! Deterministic byte-granular replay and semantic state indexing.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    slice,
    time::{Duration, Instant},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, ArtifactSourceRange, append_artifact_root, collect_artifact_members,
        commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
        sync_directory, verify_artifact_members, write_secure,
    },
    control::{ControlCommand, ControlData, request},
    event::{Event, OperationStatus},
    event_log::EventLog,
    ghostty_vt::{
        CellState, CellStyle, ColorSpec, ColorState, CursorState, DirtyState, Format, GhosttyVt,
        ModeState, RowState, SemanticState, TerminalMetadata, supported_geometry_cell_count,
    },
    session::validate_completed_log,
    timeline::{SourceRange, resolve_source_range, timeline_event},
    vt_framer::{VtFramer, VtToken},
};

mod delta;
mod engine;
mod index;
mod model;

use delta::state_delta;
#[cfg(test)]
use engine::{
    DEFAULT_REPLAY_LIMITS, ReplayLimits, replay_with, replay_with_limits,
    replay_with_seed_and_limits,
};
pub use engine::{
    RenderedStateFormats, StateInspectionWithFormats, format_state, inspect, inspect_many,
    inspect_with_formats, replay, semantic_state_hash,
};
pub(crate) use engine::{
    ReplaySeed, inspect_latest_presented_fast, visit_states, visit_states_range,
    visit_states_range_while, visit_states_seeded_while,
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
pub use model::*;

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

#[cfg(test)]
mod tests;
