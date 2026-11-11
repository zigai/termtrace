//! Hash-chained, append-only lifecycle history: the aggregate's source of truth.

use crate::{
    artifact::{TmuxEvent, TmuxEventRecord, TmuxJournalBoundary, TMUX_JOURNAL_VERSION},
    error::{Error, Result},
};
use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
};

/// Journal filename relative to an aggregate recording directory.
pub const JOURNAL_NAME: &str = "lifecycle.jsonl";
pub(crate) const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) previous_hash: String,
    pub(crate) event: TmuxEventRecord,
    pub(crate) hash: String,
}

/// A record written by [`append`] with its byte position in the journal.
pub(crate) struct Written {
    pub(crate) record: Record,
    pub(crate) offset: u64,
    pub(crate) length: u64,
}

pub(crate) fn record_hash(previous: &str, event: &TmuxEventRecord) -> Result<String> {
    let bytes = serde_json::to_vec(&(previous, event))
        .map_err(|error| return Error::json(JOURNAL_NAME, error))?;

    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

/// Discard any uncommitted suffix, then append `events` sequenced after `boundary`.
///
/// The caller holds the aggregate manifest lock and publishes the returned boundary.
pub(crate) fn append(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    events: Vec<TmuxEvent>,
    observed_unix_ns: u64,
) -> Result<(Vec<Written>, TmuxJournalBoundary)> {
    check_version(boundary)?;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(|error| return Error::filesystem(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| return Error::filesystem(path, error))?;

    if !metadata.is_file() || metadata.len() < boundary.bytes {
        return Err(Error::invalid(
            "tmux lifecycle journal is not a complete regular file",
        ));
    }

    let truncated = metadata.len() > boundary.bytes;
    if truncated {
        file.set_len(boundary.bytes)
            .map_err(|error| return Error::filesystem(path, error))?;
    }

    file.seek(SeekFrom::Start(boundary.bytes))
        .map_err(|error| return Error::filesystem(path, error))?;

    let mut next = boundary.clone();
    let mut written = Vec::with_capacity(events.len());

    for event in events {
        let event = TmuxEventRecord {
            sequence: next
                .sequence
                .checked_add(1)
                .ok_or_else(|| return Error::invalid("tmux lifecycle sequence overflow"))?,
            observed_unix_ns,
            event,
        };
        let hash = record_hash(&next.hash, &event)?;
        let record = Record {
            previous_hash: next.hash.clone(),
            event,
            hash,
        };
        let mut bytes =
            serde_json::to_vec(&record).map_err(|error| return Error::json(path, error))?;
        bytes.push(b'\n');
        let length = u64::try_from(bytes.len()).map_err(|source| {
            return Error::invalid_with_source("tmux lifecycle record is too large", source);
        })?;

        if length > MAX_RECORD_BYTES {
            return Err(Error::invalid(
                "tmux lifecycle record exceeds its frame limit",
            ));
        }

        file.write_all(&bytes)
            .map_err(|error| return Error::filesystem(path, error))?;

        let offset = next.bytes;
        next.bytes = offset
            .checked_add(length)
            .ok_or_else(|| return Error::invalid("tmux lifecycle byte offset overflow"))?;

        next.sequence = record.event.sequence;
        next.hash.clone_from(&record.hash);
        written.push(Written {
            record,
            offset,
            length,
        });
    }

    if truncated || !written.is_empty() {
        file.sync_all()
            .map_err(|error| return Error::filesystem(path, error))?;
    }

    return Ok((written, next));
}

/// Validate a committed prefix while visiting each event with bounded memory.
/// Uncommitted suffix bytes are deliberately excluded.
///
/// # Errors
/// Returns invalid frames, broken hash chains, and boundary mismatches.
pub fn visit(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    mut visitor: impl FnMut(&TmuxEventRecord) -> Result<()>,
) -> Result<()> {
    return visit_records(path, boundary, |written| {
        return visitor(&written.record.event);
    });
}

pub(crate) fn visit_records(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    mut visitor: impl FnMut(&Written) -> Result<()>,
) -> Result<()> {
    check_version(boundary)?;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path);

    let file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && boundary.bytes == 0 => {
            return Ok(());
        }
        Err(error) => return Err(Error::filesystem(path, error)),
    };

    let metadata = file
        .metadata()
        .map_err(|error| return Error::filesystem(path, error))?;

    if !metadata.is_file() || metadata.len() < boundary.bytes {
        return Err(Error::invalid(
            "tmux lifecycle journal committed prefix is missing",
        ));
    }

    let mut reader = BufReader::new(file.take(boundary.bytes));
    let mut sequence = 0_u64;
    let mut hash = String::new();
    let mut offset = 0_u64;

    loop {
        let mut bytes = Vec::new();
        let count = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|error| return Error::filesystem(path, error))?;

        if count == 0 {
            break;
        }

        let length = u64::try_from(count).map_err(|source| {
            return Error::invalid_with_source("tmux record length overflow", source);
        })?;

        if length > MAX_RECORD_BYTES || bytes.last() != Some(&b'\n') {
            return Err(Error::invalid(
                "tmux lifecycle journal has an invalid frame",
            ));
        }

        let record: Record =
            serde_json::from_slice(&bytes).map_err(|error| return Error::json(path, error))?;

        if sequence.checked_add(1) != Some(record.event.sequence)
            || record.previous_hash != hash
            || record.hash != record_hash(&hash, &record.event)?
        {
            return Err(Error::invalid("tmux lifecycle journal chain differs"));
        }

        sequence = record.event.sequence;
        hash.clone_from(&record.hash);
        visitor(&Written {
            record,
            offset,
            length,
        })?;

        offset = offset
            .checked_add(length)
            .ok_or_else(|| return Error::invalid("tmux journal offset overflow"))?;
    }

    if sequence != boundary.sequence || hash != boundary.hash {
        return Err(Error::invalid("tmux lifecycle journal boundary differs"));
    }
    return Ok(());
}

/// Copy exactly the validated committed history into a new evidence member.
///
/// # Errors
/// Returns invalid history, a concurrent change, or filesystem failures.
pub fn copy_committed(
    source: &Path,
    destination: &Path,
    boundary: &TmuxJournalBoundary,
) -> Result<()> {
    visit(source, boundary, |_| return Ok(()))?;

    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(source)
        .map_err(|error| return Error::filesystem(source, error))?
        .take(boundary.bytes);

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|error| return Error::filesystem(destination, error))?;
    let count = io::copy(&mut input, &mut output)
        .map_err(|error| return Error::filesystem(destination, error))?;

    if count != boundary.bytes {
        return Err(Error::invalid(
            "tmux lifecycle journal changed during export",
        ));
    }

    output
        .sync_all()
        .map_err(|error| return Error::filesystem(destination, error))?;

    return visit(destination, boundary, |_| return Ok(()));
}

fn check_version(boundary: &TmuxJournalBoundary) -> Result<()> {
    if boundary.version != TMUX_JOURNAL_VERSION {
        return Err(Error::invalid(format!(
            "unsupported tmux lifecycle journal version {}",
            boundary.version
        )));
    }
    return Ok(());
}

#[cfg(test)]
mod tests;
