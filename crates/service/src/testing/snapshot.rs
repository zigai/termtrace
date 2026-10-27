use super::*;
use crate::assertions::{masked_snapshot, MaskedSnapshotOptions};
use nix::{
    libc::{O_DIRECTORY, O_NONBLOCK},
    sys::stat::{mkdirat, Mode},
};

/// Typed operational or expectation failure from the Rust test helper.
#[derive(Debug)]
pub enum TerminalAssertionError {
    /// Invalid helper arguments; never a negatable mismatch.
    Validation(String),
    /// The attempt was already finalized.
    SessionUnavailable,
    /// Capturing or persisting evidence failed.
    Operation(Error),
    /// A valid runner-neutral expectation did not match.
    Assertion(AssertionError),
}

impl fmt::Display for TerminalAssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(reason) => return f.write_str(reason),
            Self::SessionUnavailable => {
                return f.write_str("terminal test session is already finalized");
            }
            Self::Operation(error) => return error.fmt(f),
            Self::Assertion(error) => return error.fmt(f),
        }
    }
}

impl StdError for TerminalAssertionError {}

impl From<Error> for TerminalAssertionError {
    fn from(error: Error) -> Self {
        return Self::Operation(error);
    }
}

impl From<AssertionError> for TerminalAssertionError {
    fn from(error: AssertionError) -> Self {
        return Self::Assertion(error);
    }
}

pub(super) fn path_for(
    snapshot_root: &Path,
    name: &str,
) -> StdResult<PathBuf, TerminalAssertionError> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

    if valid {
        return Ok(snapshot_root.join(format!("{name}.json")));
    }
    return Err(TerminalAssertionError::Validation(format!(
        "terminal_snapshot_name: expected 1-128 ASCII letters, digits, '.', '_', or '-'; got {}",
        bounded_text(name)
    )));
}

/// Compare or explicitly update one deterministic frame snapshot file.
///
/// # Errors
/// Returns an error when the frame cannot be normalized, the snapshot path has no parent, or the
/// snapshot cannot be written or read or exceeds 4 MiB, and a mismatch when the reviewed snapshot
/// is missing or differs.
#[allow(
    clippy::verbose_file_reads,
    reason = "reviewed snapshots must be opened with no-follow semantics"
)]
pub fn assert_snapshot_file(
    frame: &ScreenFrame,
    path: &Path,
    profile: SnapshotProfile,
    options: ScreenSnapshotOptions,
    update: bool,
) -> StdResult<(), TerminalAssertionError> {
    let actual = normalized_snapshot(frame, profile, options)?;

    return compare_snapshot_document(frame, path, actual, update);
}

/// Compare or explicitly update a separate masked version-one baseline.
///
/// Masks are stability exclusions, not redaction. Validation completes before filesystem I/O.
///
/// # Errors
/// Returns non-negatable validation/operational errors or a bounded comparison mismatch.
pub fn assert_masked_snapshot_file(
    frame: &ScreenFrame,
    path: &Path,
    options: &MaskedSnapshotOptions,
    update: bool,
) -> StdResult<(), TerminalAssertionError> {
    let actual = masked_snapshot(frame, options)?;

    return compare_snapshot_document(frame, path, actual, update);
}

const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

fn compare_snapshot_document(
    frame: &ScreenFrame,
    path: &Path,
    mut actual: String,
    update: bool,
) -> StdResult<(), TerminalAssertionError> {
    use nix::fcntl::{Flock, FlockArg};
    use std::os::fd::AsRawFd as _;

    actual.push('\n');
    if u64::try_from(actual.len()).unwrap_or(u64::MAX) > MAX_SNAPSHOT_BYTES {
        return Err(testing_error("normalized snapshot exceeds 4 MiB").into());
    }

    let parent = path
        .parent()
        .filter(|parent| return !parent.as_os_str().is_empty())
        .unwrap_or_else(|| return Path::new("."));
    let filename = path
        .file_name()
        .ok_or_else(|| return testing_error("snapshot path has no filename"))?;

    let parent = match open_snapshot_parent(parent, update) {
        Ok(parent) => parent,
        Err(Error::Filesystem { source, .. })
            if !update && source.kind() == io::ErrorKind::NotFound =>
        {
            return Err(snapshot_mismatch(
                frame,
                format!("reviewed snapshot {}", path.display()),
                "snapshot file is missing".to_owned(),
                String::new(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    let anchored_parent = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let target = anchored_parent.join(filename);
    // Reject concurrent publishers rather than silently racing them. Locks are process-wide and
    // cross-process; all updates in this directory share the directory inode lock.
    let _lock = if update {
        Some(
            Flock::lock(
                parent.try_clone().map_err(|source| {
                    return Error::Filesystem {
                        path: path.to_path_buf(),
                        source,
                    };
                })?,
                FlockArg::LockExclusiveNonblock,
            )
            .map_err(|(_file, error)| {
                return testing_error(format!("snapshot update collision: {error}"));
            })?,
        )
    } else {
        None
    };

    match fs_err::symlink_metadata(&target) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(testing_error("snapshot target is not a regular file").into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if !update {
                return Err(snapshot_mismatch(
                    frame,
                    format!("reviewed snapshot {}", path.display()),
                    "snapshot file is missing".to_owned(),
                    String::new(),
                ));
            }
        }
        Err(source) => {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source,
            }
            .into())
        }
    }

    if update {
        // NamedTempFile unlinks on every failure path. Publication is atomic on the held parent
        // inode, even if an ancestor is renamed while this call is in progress.
        let temporary = tempfile::Builder::new()
            .prefix(".snapshot-")
            .tempfile_in(&anchored_parent)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            })?;

        let mut file = temporary.as_file();
        file.write_all(actual.as_bytes())
            .and_then(|()| return file.sync_all())
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            })?;
        fs_err::rename(temporary.path(), &target).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        parent.sync_all().map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;

        return Ok(());
    }

    let (file, byte_count) = open_regular_file_nofollow(&target)?;
    if byte_count > MAX_SNAPSHOT_BYTES {
        return Err(testing_error("reviewed snapshot exceeds 4 MiB").into());
    }

    let mut expected = String::new();
    file.take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_string(&mut expected)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;

    if u64::try_from(expected.len()).unwrap_or(u64::MAX) > MAX_SNAPSHOT_BYTES {
        return Err(testing_error("reviewed snapshot exceeds 4 MiB").into());
    }

    if expected == actual {
        return Ok(());
    }

    let excerpt = format!(
        "expected:\n{}\nactual:\n{}",
        bounded_text(&expected),
        bounded_text(&actual)
    );

    return Err(snapshot_mismatch(
        frame,
        format!("reviewed snapshot {}", path.display()),
        "normalized frame differs".to_owned(),
        excerpt,
    ));
}

fn open_snapshot_parent(path: &Path, create: bool) -> Result<File> {
    use std::os::fd::AsRawFd as _;
    use std::path::Component;
    if path
        .components()
        .any(|component| return matches!(component, Component::ParentDir))
    {
        return Err(testing_error("snapshot parent traversal is invalid"));
    }

    let open = |path: &Path| {
        return OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW | O_CLOEXEC | O_DIRECTORY)
            .open(path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            });
    };
    let mut directory = open(if path.is_absolute() { Path::new("/") } else { Path::new(".") })?;

    for component in path.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::ParentDir) {
                return Err(testing_error("snapshot parent traversal is invalid"));
            }

            continue;
        };
        let child = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
        if create && let Err(error) = fs_err::symlink_metadata(&child) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(Error::Filesystem {
                    path: child,
                    source: error,
                });
            }

            mkdirat(&directory, name, Mode::S_IRWXU).map_err(|source| {
                return Error::Filesystem {
                    path: child.clone(),
                    source: source.into(),
                };
            })?;
            directory.sync_all().map_err(|source| {
                return Error::Filesystem {
                    path: child.clone(),
                    source,
                };
            })?;
        }

        directory = open(&child)?;
    }

    return Ok(directory);
}

fn snapshot_mismatch(
    frame: &ScreenFrame,
    expected: String,
    actual: String,
    excerpt: String,
) -> TerminalAssertionError {
    return TerminalAssertionError::Assertion(AssertionError::Mismatch(ExpectationMismatchError {
        assertion: "terminal_snapshot",
        expected: bounded_text(&expected),
        actual: bounded_text(&actual),
        excerpt: bounded_text(&excerpt),
        frame: Some(Box::new(AssertionFrameIdentity {
            state_hash: frame.state_hash.clone(),
            projection_hash: frame.frame_hash.clone(),
            presented: frame.presented,
        })),
        truncated: expected.len() > MAX_SECONDARY_FAILURE_CHARS
            || actual.len() > MAX_SECONDARY_FAILURE_CHARS
            || excerpt.len() > MAX_SECONDARY_FAILURE_CHARS,
    }));
}

pub(super) fn bounded_text(value: &str) -> String {
    let end = value.floor_char_boundary(MAX_SECONDARY_FAILURE_CHARS);

    return value.get(..end).unwrap_or_default().to_owned();
}

pub(super) fn open_regular_file_nofollow(path: &Path) -> Result<(File, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    if !metadata.is_file() {
        return Err(testing_error("evidence member is not a regular file"));
    }
    return Ok((file, metadata.len()));
}

pub(super) fn testing_error(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
        source: None,
    };
}
