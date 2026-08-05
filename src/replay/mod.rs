//! Deterministic byte-granular replay and semantic state indexing.

use std::{
    collections::HashMap,
    fmt::Write as _,
    io::{Result as IoResult, Write},
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
        CellState, CellStyle, ColorState, CursorState, DirtyState, Format, GhosttyVt, ModeState,
        SemanticState, TerminalMetadata, supported_geometry_cell_count,
    },
    session::validate_completed_log,
    timeline::{SourceRange, resolve_source_range},
};

mod delta;
mod engine;
mod index;
mod model;

use delta::state_delta;
use engine::replay_with;
pub(crate) use engine::visit_states;
#[cfg(test)]
use engine::{DEFAULT_REPLAY_LIMITS, ReplayLimits, replay_with_limits};
pub use engine::{
    RenderedStateFormats, StateInspectionWithFormats, format_state, inspect, inspect_many,
    inspect_with_formats, replay,
};
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
