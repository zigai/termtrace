//! Owner-only, durable, no-replace artifact filesystem helpers.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::File,
    io::{BufRead as _, BufReader, BufWriter, Error as IoError, ErrorKind, Read as _, Write as _},
    os::fd::{AsFd, OwnedFd},
    path::{Component, Path, PathBuf},
    thread,
};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, OFlag, OpenHow, RenameFlags, ResolveFlag, open, openat2, renameat2},
    sys::stat::{Mode, SFlag, fstat, mkdirat},
    unistd::{Uid, fsync},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result};

mod model;
mod roots;
mod secure_fs;
mod verification;

pub use model::*;
pub use roots::{append_artifact_root, read_artifact_roots};
pub use secure_fs::write_secure;
pub(crate) use secure_fs::{
    commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
    sync_directory, sync_parent_directory,
};
use secure_fs::{
    nix_filesystem, open_directory_path, open_file_at, open_parent_directory, safe_file_name,
    sync_fd, validate_private_directory_fd, validate_private_regular_file_fd,
};
use verification::normalized_relative_path;
pub use verification::{
    collect_artifact_members, verify_artifact_members, verify_session_artifacts,
};
pub(crate) use verification::{
    collect_artifact_members_bounded, hash_regular_file, open_regular_file_nofollow,
};

/// Relative append-only artifact-root catalog path.
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
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests;
