use super::*;

/// Typed operational or expectation failure from the Rust test helper.
#[derive(Debug)]
pub enum TerminalAssertionError {
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

pub(super) fn snapshot_path(
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

    return Err(TerminalAssertionError::Assertion(AssertionError::Mismatch(
        ExpectationMismatch {
            assertion: "terminal_snapshot_name",
            expected: "1-128 ASCII letters, digits, '.', '_', or '-'".to_owned(),
            actual: bounded_text(name),
            excerpt: String::new(),
            frame: None,
            truncated: name.len() > MAX_SECONDARY_FAILURE_CHARS,
        },
    )));
}

/// Compare or explicitly update one deterministic frame snapshot file.
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
    const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

    let mut actual = normalized_snapshot(frame, profile, options)?;
    actual.push('\n');

    let parent = path
        .parent()
        .ok_or_else(|| return testing_error("snapshot path has no parent"))?;

    if update {
        if !parent.exists() {
            create_private_directory(parent).map_err(Error::from)?;
        }

        let temporary = parent.join(format!(".snapshot-{}.tmp", Uuid::now_v7()));
        write_secure(&temporary, actual.as_bytes()).map_err(Error::from)?;
        fs_err::rename(&temporary, path).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;

        sync_directory(parent).map_err(Error::from)?;

        return Ok(());
    }

    if !path.exists() {
        return Err(snapshot_mismatch(
            frame,
            format!("reviewed snapshot {}", path.display()),
            "snapshot file is missing".to_owned(),
            String::new(),
        ));
    }

    let (mut file, byte_count) = open_regular_file_nofollow(path)?;
    if byte_count > MAX_SNAPSHOT_BYTES {
        return Err(TerminalAssertionError::Operation(testing_error(
            "reviewed snapshot exceeds 4 MiB",
        )));
    }

    let mut expected = String::new();
    file.read_to_string(&mut expected).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

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

fn snapshot_mismatch(
    frame: &ScreenFrame,
    expected: String,
    actual: String,
    excerpt: String,
) -> TerminalAssertionError {
    return TerminalAssertionError::Assertion(AssertionError::Mismatch(ExpectationMismatch {
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
        .custom_flags(O_NOFOLLOW | O_CLOEXEC)
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
    };
}
