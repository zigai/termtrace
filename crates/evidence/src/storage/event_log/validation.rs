use std::{
    fs,
    io::{BufRead, Error as IoError, ErrorKind},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
};

use blake3::Hash;
use nix::unistd::Uid;
use termtrace_model::{
    execution::event::{EVENT_LOG_FORMAT, EVENT_LOG_VERSION, Event, EventLogHeader},
    terminal::state::{MAX_TERMINAL_CELLS, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS},
};

use crate::storage::{Error, Result};

pub(super) const OWNER_DIRECTORY_MODE: u32 = 0o700;
pub(super) const OWNER_FILE_MODE: u32 = 0o600;
pub(super) const MAX_LINE_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn validate_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "event log has no parent"),
        };
    })?;
    let metadata = fs::metadata(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    if !metadata.is_dir() {
        return Err(Error::Filesystem {
            path: parent.to_path_buf(),
            source: IoError::new(
                ErrorKind::NotADirectory,
                "event log parent is not a directory",
            ),
        });
    }
    let mode = metadata.permissions().mode() & 0o777;
    let owner_uid = metadata.uid();
    if owner_uid != Uid::effective().as_raw() || mode != OWNER_DIRECTORY_MODE {
        return Err(Error::InsecureArtifactDirectory {
            path: parent.to_path_buf(),
            owner_uid,
            mode,
        });
    }
    return Ok(());
}

pub(super) fn read_line(
    reader: &mut impl BufRead,
    path: &Path,
    line: u64,
) -> Result<Option<Vec<u8>>> {
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(invalid_log(
                    path,
                    line,
                    "truncated line without newline terminator",
                ))
            };
        }
        let consumed = available
            .iter()
            .position(|byte| return *byte == b'\n')
            .map_or(available.len(), |position| return position + 1);
        if u64::try_from(bytes.len().saturating_add(consumed)).unwrap_or(u64::MAX) > MAX_LINE_BYTES
        {
            return Err(invalid_log(path, line, "line exceeds 64 MiB limit"));
        }
        let (chunk, _) = available.split_at(consumed);
        bytes.extend_from_slice(chunk);
        let complete = chunk.last() == Some(&b'\n');
        reader.consume(consumed);
        if complete {
            bytes.pop();
            return Ok(Some(bytes));
        }
    }
}

pub(super) fn parse_json_line<T>(path: &Path, line: u64, bytes: &[u8]) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let json_path = error.path().to_string();
        let source = error.into_inner();
        return Error::InvalidEventLog {
            path: path.to_path_buf(),
            line,
            reason: format!("JSON decoding failed at `{json_path}`: {source}"),
        };
    });
}

pub(super) fn validate_header(path: &Path, header: &EventLogHeader) -> Result<()> {
    if header.format != EVENT_LOG_FORMAT {
        return Err(invalid_log(path, 1, "unsupported format identifier"));
    }
    if header.version != EVENT_LOG_VERSION {
        return Err(invalid_log(
            path,
            1,
            format!("unsupported format version {}", header.version),
        ));
    }
    if supported_geometry_cell_count(header.cols, header.rows).is_none() {
        return Err(invalid_log(path, 1, geometry_limit_reason("initial")));
    }
    if header.session_manifest_hash.len() != 64
        || !header
            .session_manifest_hash
            .bytes()
            .all(|byte| return byte.is_ascii_hexdigit())
    {
        return Err(invalid_log(
            path,
            1,
            "session manifest hash must be a 64-character hexadecimal BLAKE3 digest",
        ));
    }
    if header.term.is_empty() {
        return Err(invalid_log(path, 1, "terminal identity must not be empty"));
    }
    if header.colors.source.is_empty() {
        return Err(invalid_log(
            path,
            1,
            "terminal color source must not be empty",
        ));
    }
    if !header.colors.palette.is_empty() && header.colors.palette.len() != 256 {
        return Err(invalid_log(
            path,
            1,
            "terminal palette must be empty or contain exactly 256 colors",
        ));
    }
    return Ok(());
}

fn supported_geometry_cell_count(cols: u16, rows: u16) -> Option<usize> {
    if cols == 0 || rows == 0 || cols > MAX_TERMINAL_COLUMNS || rows > MAX_TERMINAL_ROWS {
        return None;
    }
    let cells = u32::from(cols).checked_mul(u32::from(rows))?;
    if cells > MAX_TERMINAL_CELLS {
        return None;
    }
    return usize::try_from(cells).ok();
}

fn geometry_limit_reason(kind: &str) -> String {
    return format!(
        "{kind} terminal geometry exceeds limits of {MAX_TERMINAL_COLUMNS} columns, {MAX_TERMINAL_ROWS} rows, and {MAX_TERMINAL_CELLS} cells"
    );
}

pub(super) fn validate_event_geometry(path: &Path, line: u64, event: &Event) -> Result<()> {
    if let Event::ResizeIntent { cols, rows, .. } = event
        && supported_geometry_cell_count(*cols, *rows).is_none()
    {
        return Err(invalid_log(path, line, geometry_limit_reason("resize")));
    }
    return Ok(());
}

pub(super) fn chained_hash(previous: Hash, body: &[u8]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(previous.as_bytes());
    hasher.update(body);
    return hasher.finalize();
}

pub(super) fn invalid_log(path: &Path, line: u64, reason: impl Into<String>) -> Error {
    return Error::InvalidEventLog {
        path: path.to_path_buf(),
        line,
        reason: reason.into(),
    };
}
