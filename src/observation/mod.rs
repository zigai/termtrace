//! Stable, truth-labeled terminal observation envelopes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zstd::stream::{decode_all, encode_all, read::Decoder};

use std::{
    fs::{File, OpenOptions},
    io::{BufReader, ErrorKind, Read as _},
    os::unix::ffi::OsStrExt as _,
    os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::SystemTime,
};

use nix::libc::O_NOFOLLOW;

use crate::{
    Error, Result,
    artifact::{create_private_directory, sync_directory, write_secure},
    control::{ControlCommand, ControlData, request},
    event::{ByteString, Event},
    event_log::{CanonicalBoundary, EventLog},
    ghostty_vt::{Format, GhosttyVt, ModeState, SemanticState},
    replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION, StateInspection, StateSelector, inspect},
    session::{SessionCompletion, read_completion, read_manifest, validate_completed_log},
};

mod ansi;
mod cache;
mod model;
mod session;

pub use ansi::{format_ansi_stream, observe_ansi_stream, semantic_state_from_ansi};
pub use cache::{
    ReplayCachePruneReport, ReplayCacheStatus, ReplayCheckpoint, prune_replay_cache,
    replay_cache_status,
};
use cache::{load_checkpoint, publish_checkpoint};
use model::observation_error;
pub use model::{
    CaptureQuality, FrameEnvelope, Observation, ObservationFrame, ObservationIntegrity,
    ObservationResolution, ObservationSourceKind, StatePoint, TruthLayer,
};
pub(crate) use model::{frame_from_state, frame_hash};
pub(crate) use session::{completed_observation_from_inspection, state_selector};
pub use session::{
    observe_one_off_session, observe_session, observe_session_with_options,
    observe_validated_prefix, semantic_state_for_session, semantic_state_with_hash_for_session,
};
