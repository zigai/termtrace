//! Deterministic, sensitivity-labeled evidence handoff directories.

use std::{
    collections::BTreeSet,
    fs::FileType,
    io::{self, Read as _, Write as _},
    os::unix::fs::FileTypeExt as _,
    path::{Component, Path, PathBuf},
    str,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, ArtifactSensitivity, collect_artifact_members_bounded, commit_directory,
        create_private_directory, finish_writer, hash_regular_file, open_regular_file_nofollow,
        secure_file, secure_writer, sync_directory, write_secure,
    },
    event::{ByteString, EventProjection},
    event_log::EventLog,
    observation::{
        CaptureQuality, Observation, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer, frame_from_state, frame_hash,
        observe_session,
    },
    replay::{StateSelector, inspect},
    session::{
        SessionCompletion, SessionManifest, VerificationLevel, VerificationReport, read_manifest,
        validate_completed_log, verify_session,
    },
};

mod create;
mod model;
mod plan;
mod tar;
mod verify;

pub use create::create;
use create::top_level_regular_file_names;
pub use model::*;
pub use plan::plan;
use plan::*;
pub use tar::{archive, verify_archive};
#[cfg(test)]
use tar::{extract_archive, write_checksum, write_octal, write_tar_path};
pub use verify::verify;
#[cfg(test)]
use verify::{read_bounded_regular_file, verify_canonical_payload_matches_source};

const MAX_BUNDLE_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BUNDLE_STRUCTURED_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BUNDLE_MEMBERS: usize = 10_000;
const MAX_BUNDLE_TREE_ENTRIES: usize = MAX_BUNDLE_MEMBERS + 2;

fn json_error(path: &str, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path: PathBuf::from(path),
        json_path: "$".to_owned(),
        source,
    };
}

fn bundle_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests;
