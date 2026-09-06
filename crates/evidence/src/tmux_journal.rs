//! Full lifecycle history with a bounded recent tail in the aggregate snapshot.

use std::{
    fs::OpenOptions,
    io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
};

use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::{Deserialize, Serialize};
use termtrace_model::artifact::tmux::{
    TmuxJournalBoundary, TmuxLifecycleEvent, TmuxRecordingManifest,
};

use crate::{Error, Result, tmux_catalog::TmuxCatalog};

/// Journal filename relative to an aggregate recording directory.
pub const JOURNAL_NAME: &str = "lifecycle.jsonl";
const RECENT_EVENTS: usize = 128;
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) previous_hash: String,
    pub(crate) event: TmuxLifecycleEvent,
    pub(crate) hash: String,
}

pub(crate) fn record_hash(previous: &str, event: &TmuxLifecycleEvent) -> Result<String> {
    let bytes = serde_json::to_vec(&(previous, event))
        .map_err(|error| return json_error(Path::new(JOURNAL_NAME), error))?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

/// Persist newly observed events before publishing the caller's snapshot.
///
/// The caller must hold the aggregate manifest lock through snapshot publication.
/// Any suffix left by a failed publication is discarded at the last committed
/// boundary. A successful return still requires atomic snapshot publication and
/// a directory sync by the caller.
pub fn append(directory: &Path, manifest: &mut TmuxRecordingManifest) -> Result<()> {
    let mut catalog = TmuxCatalog::for_append(directory, manifest)?;
    if manifest.catalog.is_none() {
        seed_catalog_history(manifest)?;
    }
    let path = directory.join(JOURNAL_NAME);
    let mut boundary = manifest
        .lifecycle_journal
        .clone()
        .unwrap_or(TmuxJournalBoundary {
            bytes: 0,
            sequence: 0,
            hash: String::new(),
        });
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&path)
        .map_err(|error| return Error::filesystem(&path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| return Error::filesystem(&path, error))?;
    if !metadata.is_file() || metadata.len() < boundary.bytes {
        return Err(Error::invalid(
            "tmux lifecycle journal is not a complete regular file",
        ));
    }
    file.set_len(boundary.bytes)
        .map_err(|error| return Error::filesystem(&path, error))?;
    file.seek(SeekFrom::Start(boundary.bytes))
        .map_err(|error| return Error::filesystem(&path, error))?;
    let mut indexed = Vec::new();
    for event in &manifest.events {
        if event.sequence <= boundary.sequence {
            continue;
        }
        if boundary.sequence.checked_add(1) != Some(event.sequence) {
            return Err(Error::invalid("tmux lifecycle sequence is discontinuous"));
        }
        let hash = record_hash(&boundary.hash, event)?;
        let record = Record {
            previous_hash: boundary.hash.clone(),
            event: event.clone(),
            hash: hash.clone(),
        };
        let mut bytes =
            serde_json::to_vec(&record).map_err(|error| return json_error(&path, error))?;
        bytes.push(b'\n');
        let count = u64::try_from(bytes.len())
            .map_err(|_| return Error::invalid("tmux lifecycle record is too large"))?;
        if count > MAX_RECORD_BYTES {
            return Err(Error::invalid(
                "tmux lifecycle record exceeds its frame limit",
            ));
        }
        file.write_all(&bytes)
            .map_err(|error| return Error::filesystem(&path, error))?;
        indexed.push((record, boundary.bytes, count));
        boundary.bytes = boundary
            .bytes
            .checked_add(count)
            .ok_or_else(|| return Error::invalid("tmux lifecycle byte offset overflow"))?;
        boundary.sequence = event.sequence;
        boundary.hash = hash;
    }
    file.sync_all()
        .map_err(|error| return Error::filesystem(&path, error))?;
    manifest.catalog = Some(catalog.append(&indexed, &boundary)?);
    manifest.lifecycle_journal = Some(boundary);
    let discard = manifest.events.len().saturating_sub(RECENT_EVENTS);
    manifest.events.drain(..discard);
    while manifest.track_history.len() > 128 {
        manifest.track_history.pop_first();
    }
    manifest
        .coverage_gaps
        .retain(|gap| return gap.resolved_unix_ns.is_none());
    return Ok(());
}

fn seed_catalog_history(manifest: &mut TmuxRecordingManifest) -> Result<()> {
    let mut sequence = manifest.events.last().map_or_else(
        || {
            return manifest
                .lifecycle_journal
                .as_ref()
                .map_or(0, |boundary| return boundary.sequence);
        },
        |event| return event.sequence,
    );
    let mut append = |kind: &str,
                      pane_id: Option<String>,
                      value: String,
                      observed_unix_ns: u128|
     -> Result<()> {
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("tmux lifecycle sequence overflow"))?;
        manifest.events.push(TmuxLifecycleEvent {
            sequence,
            observed_unix_ns,
            kind: kind.to_owned(),
            session_id: None,
            window_id: None,
            pane_id,
            value: Some(value),
        });
        return Ok(());
    };
    for track in manifest.track_history.values() {
        append(
            "pane_track_linked",
            Some(track.pane_id.clone()),
            serde_json::to_string(track)
                .map_err(|error| return json_error(Path::new(JOURNAL_NAME), error))?,
            track.linked_unix_ns,
        )?;
    }
    for gap in &manifest.coverage_gaps {
        let mut opened = gap.clone();
        opened.resolved_unix_ns = None;
        append(
            "coverage_gap_opened",
            gap.pane_id.clone(),
            serde_json::to_string(&opened)
                .map_err(|error| return json_error(Path::new(JOURNAL_NAME), error))?,
            gap.first_observed_unix_ns,
        )?;
        if let Some(resolved) = gap.resolved_unix_ns {
            append(
                "coverage_gap_resolved",
                gap.pane_id.clone(),
                serde_json::to_string(gap)
                    .map_err(|error| return json_error(Path::new(JOURNAL_NAME), error))?,
                resolved,
            )?;
        }
    }
    return Ok(());
}

/// Validate a committed prefix while visiting each event with bounded memory.
/// Uncommitted suffix bytes are deliberately excluded.
pub fn visit(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    mut visitor: impl FnMut(&TmuxLifecycleEvent) -> Result<()>,
) -> Result<()> {
    return visit_records(path, boundary, |record, _, _| return visitor(&record.event));
}

pub(crate) fn visit_records(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    mut visitor: impl FnMut(&Record, u64, u64) -> Result<()>,
) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(|error| return Error::filesystem(path, error))?;
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
        if u64::try_from(count).unwrap_or(u64::MAX) > MAX_RECORD_BYTES
            || bytes.last() != Some(&b'\n')
        {
            return Err(Error::invalid(
                "tmux lifecycle journal has an invalid frame",
            ));
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|error| return json_error(path, error))?;
        if sequence.checked_add(1) != Some(record.event.sequence)
            || record.previous_hash != hash
            || record.hash != record_hash(&hash, &record.event)?
        {
            return Err(Error::invalid("tmux lifecycle journal chain differs"));
        }
        sequence = record.event.sequence;
        hash = record.hash.clone();
        let count = u64::try_from(count)
            .map_err(|_| return Error::invalid("tmux record length overflow"))?;
        visitor(&record, offset, count)?;
        offset = offset
            .checked_add(count)
            .ok_or_else(|| return Error::invalid("tmux journal offset overflow"))?;
    }
    if sequence != boundary.sequence || hash != boundary.hash {
        return Err(Error::invalid("tmux lifecycle journal boundary differs"));
    }
    return Ok(());
}

/// Copy exactly the validated committed history into a new evidence member.
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

fn json_error(path: &Path, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path: path.to_owned(),
        json_path: String::new(),
        source,
    };
}
