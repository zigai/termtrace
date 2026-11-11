//! Shared source authentication for finite canonical pages; no suffix trust from caller cursors.

use crate::{Error, Result};
use std::{
    io::{self, BufRead as _, BufReader, Read as _},
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
};
use termtrace_evidence::{
    event_log::{CanonicalLogFile, EventLogCursor},
    session::{manifest_hash, read_manifest},
};
use termtrace_model::{
    execution::paging::{EventPageCursor, PageSource},
    value::ByteString,
};

pub(crate) fn invalid(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
        source: None,
    };
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source,
    };
}

pub(crate) fn canonical_session(session: &Path) -> Result<PathBuf> {
    return fs_err::canonicalize(session).map_err(|source| return filesystem(session, source));
}

pub(crate) fn open(session: &Path) -> Result<(EventLogCursor, PageSource)> {
    let session = canonical_session(session)?;
    let manifest = read_manifest(&session)?;
    let path = session.join(&manifest.event_log);
    // Read the logical stream, not a physical chunk: headers may cross a segment boundary.
    let file = CanonicalLogFile::open(&path).map_err(|source| return filesystem(&path, source))?;
    let metadata = file
        .metadata()
        .map_err(|source| return filesystem(&path, source))?;

    let mut header = Vec::new();
    BufReader::new(file.take(64 * 1024 * 1024 + 1))
        .read_until(b'\n', &mut header)
        .map_err(|source| return filesystem(&path, source))?;

    if header.pop() != Some(b'\n') {
        return Err(invalid("missing or over-budget event header"));
    }

    let header_hash = manifest_hash(&header);
    let reader = EventLogCursor::open_snapshot(&path)?;
    reader.validate_live_source(metadata.dev(), metadata.ino(), &header_hash)?;

    let manifest_path = session.join("session.json");
    let manifest_bytes =
        fs_err::read(&manifest_path).map_err(|source| return filesystem(&manifest_path, source))?;

    if reader.header().session_manifest_hash != manifest_hash(&manifest_bytes) {
        return Err(invalid(
            "event header does not bind the current manifest bytes",
        ));
    }

    if reader.header().session_id != manifest.session_id {
        return Err(invalid("event header belongs to another session"));
    }

    let source = PageSource {
        session_path: ByteString::from(session.as_os_str().as_bytes()),
        session_path_display: session.display().to_string(),
        session_id: manifest.session_id,
        device: metadata.dev(),
        inode: metadata.ino(),
        header_hash,
    };

    return Ok((reader, source));
}

pub(crate) fn start(source: &PageSource) -> EventPageCursor {
    return EventPageCursor {
        source: source.clone(),
        next_event_seq: 0,
        event_hash: source.header_hash.clone(),
    };
}

pub(crate) fn check_source(source: &PageSource, cursor: &EventPageCursor) -> Result<()> {
    let expected = &cursor.source;

    if expected.session_path != source.session_path
        || expected.session_id != source.session_id
        || expected.device != source.device
        || expected.inode != source.inode
        || expected.header_hash != source.header_hash
    {
        return Err(invalid(
            "page cursor belongs to a different path, session, inode, or header",
        ));
    }
    return Ok(());
}

pub(crate) fn check_boundary(expected: &EventPageCursor, actual: &EventPageCursor) -> Result<()> {
    check_source(&actual.source, expected)?;
    if expected.next_event_seq != actual.next_event_seq || expected.event_hash != actual.event_hash
    {
        return Err(invalid(
            "page cursor does not match the validated canonical prefix",
        ));
    }
    return Ok(());
}

pub(crate) fn next(position: &mut EventPageCursor, seq: u64, hash: String) -> Result<()> {
    position.next_event_seq = seq
        .checked_add(1)
        .ok_or_else(|| return invalid("event sequence overflow"))?;

    position.event_hash = hash;
    return Ok(());
}
