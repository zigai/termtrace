use std::path::PathBuf;

use crate::storage::Error;

mod roots;
mod secure_fs;
mod verification;

#[cfg(test)]
mod tests;

pub use roots::{append_artifact_root, read_artifact_roots};
pub(crate) use secure_fs::sync_parent_directory;
pub use secure_fs::{
    commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
    sync_directory, write_secure,
};
#[cfg(test)]
pub(crate) use verification::ArtifactMemberEnvelope;
pub use verification::{
    collect_artifact_members, hash_regular_file, verify_artifact_members, verify_session_artifacts,
};

/// Session-relative name of the append-only artifact-root catalog.
pub const ARTIFACT_ROOT_LOG: &str = "artifact-roots.jsonl";
const ARTIFACT_ROOT_LOCK: &str = ".artifact-roots.lock";
const ARTIFACT_ROOT_FORMAT: &str = "termtrace-artifact-root";
const ARTIFACT_ROOT_VERSION: u32 = 1;

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn invalid_artifact(reason: impl Into<String>) -> Error {
    return Error::InvalidEvidence {
        reason: reason.into(),
    };
}
