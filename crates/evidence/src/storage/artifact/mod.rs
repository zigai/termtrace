use crate::storage::Error;
use std::{
    io::{self, Write},
    path::PathBuf,
};

mod roots;
mod secure_fs;
#[cfg(test)]
mod tests;
mod verification;

pub use roots::{append_artifact_prefix_root, append_artifact_root, read_artifact_roots};
pub(crate) use secure_fs::sync_parent_directory;
pub use secure_fs::{
    commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
    sync_directory, write_secure,
};
#[cfg(test)]
pub(crate) use verification::ArtifactMemberEnvelope;
pub use verification::{
    collect_artifact_members, hash_regular_file, open_regular_file_nofollow,
    verify_artifact_members, verify_session_artifacts,
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

/// Streaming output that refuses a write before it exceeds its byte allowance.
pub struct BudgetWriter<W> {
    inner: W,
    remaining: u64,
}

impl<W> BudgetWriter<W> {
    /// Wrap an output with a strict byte allowance.
    pub const fn new(inner: W, maximum: u64) -> Self {
        return Self {
            inner,
            remaining: maximum,
        };
    }

    /// Bytes still available to subsequent output.
    pub const fn remaining(&self) -> u64 {
        return self.remaining;
    }
}

impl<W: Write> Write for BudgetWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if u64::try_from(buf.len()).unwrap_or(u64::MAX) > self.remaining {
            return Err(io::Error::other("artifact exceeds its byte budget"));
        }

        let count = self.inner.write(buf)?;
        self.remaining -= u64::try_from(count).unwrap_or(u64::MAX);
        return Ok(count);
    }

    fn flush(&mut self) -> io::Result<()> {
        return self.inner.flush();
    }
}
