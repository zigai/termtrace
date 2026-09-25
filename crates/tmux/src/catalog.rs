//! A rebuildable SQLite index over the authoritative tmux lifecycle journal.

use crate::{
    artifact::{
        TmuxCatalogSummary, TmuxEvent, TmuxEventRecord, TmuxGapCatalogEntry, TmuxJournalBoundary,
        TmuxPaneTrack, TmuxRecordingManifest, TmuxTrackCatalogEntry,
    },
    error::{Error, Result},
    journal::{self, record_hash, visit_records, Record, Written, MAX_RECORD_BYTES},
};
use nix::{
    libc::{O_NOFOLLOW, O_NONBLOCK},
    unistd::geteuid,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension as _, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read as _, Seek as _, SeekFrom},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

/// Rebuildable index filename beside the canonical lifecycle journal.
pub const CATALOG_NAME: &str = "inventory.sqlite3";

#[derive(Serialize, Deserialize)]
struct Head {
    bytes: u64,
    sequence: u64,
    hash: String,
    counts: TmuxCatalogSummary,
}

/// A fixed-source view of the complete inventory, retaining a bounded SQLite cache.
///
/// Read views hold a SQLite snapshot. If the shared index is absent or ahead of the
/// requested manifest, a private on-disk index is rebuilt from its committed prefix.
/// Writers must hold the aggregate manifest lock through publication.
pub struct TmuxCatalog {
    connection: Connection,
    path: PathBuf,
    journal: PathBuf,
    head: Head,
}

impl TmuxCatalog {
    /// Open an inventory snapshot without modifying the shared index.
    ///
    /// # Errors
    /// Returns source, journal, index, or permission failures.
    pub fn read(directory: &Path, manifest: &TmuxRecordingManifest) -> Result<Self> {
        let path = directory.join(CATALOG_NAME);
        let connection = match checked_index_file(&path, false) {
            Ok(_) => Some(
                Connection::open_with_flags(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(|error| return sql_error(&path, error))?,
            ),
            Err(Error::Filesystem { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                None
            }
            Err(error) => return Err(error),
        };

        if let Some(connection) = connection {
            connection
                .busy_timeout(Duration::from_secs(2))
                .map_err(|error| return sql_error(&path, error))?;
            connection
                .execute_batch("PRAGMA cache_size=-1024; BEGIN DEFERRED;")
                .map_err(|error| return sql_error(&path, error))?;
            if let Some(head) = load_head(&connection, &path)?
                && matches_manifest(&head, manifest)
            {
                return Ok(Self {
                    connection,
                    path,
                    journal: directory.join(journal::JOURNAL_NAME),
                    head,
                });
            }
        }

        return Self::read_verified(directory, manifest);
    }

    /// Rebuild a private inventory from the complete committed canonical journal.
    ///
    /// This verifies the chain through its caller-owned boundary and does not
    /// trust the shared acceleration index. Use it for evidence export.
    ///
    /// # Errors
    /// Returns invalid journal chains, malformed events, and storage failures.
    pub fn read_verified(directory: &Path, manifest: &TmuxRecordingManifest) -> Result<Self> {
        let path = directory.join(CATALOG_NAME);
        let connection = Connection::open("").map_err(|error| return sql_error(&path, error))?;
        let mut catalog = Self::initialize(connection, directory)?;
        catalog.rebuild(manifest)?;

        return Ok(catalog);
    }

    /// Open the shared index under the caller's exclusive aggregate manifest lock.
    pub(crate) fn for_append(directory: &Path, manifest: &TmuxRecordingManifest) -> Result<Self> {
        let path = directory.join(CATALOG_NAME);
        checked_index_file(&path, true)?;
        let connection = Connection::open(&path).map_err(|error| return sql_error(&path, error))?;

        let mut catalog = Self::initialize(connection, directory)?;
        if !matches_manifest(&catalog.head, manifest) {
            catalog.rebuild(manifest)?;
        }

        return Ok(catalog);
    }

    fn initialize(connection: Connection, directory: &Path) -> Result<Self> {
        let path = directory.join(CATALOG_NAME);
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|error| return sql_error(&path, error))?;
        connection.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS head (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events (sequence INTEGER PRIMARY KEY, offset INTEGER NOT NULL, length INTEGER NOT NULL, hash TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS tracks (id TEXT PRIMARY KEY, pane TEXT NOT NULL, sequence INTEGER NOT NULL UNIQUE, value TEXT NOT NULL, completed INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS unfinished_tracks ON tracks(completed, sequence);
            CREATE TABLE IF NOT EXISTS gaps (sequence INTEGER PRIMARY KEY, pane TEXT, kind TEXT NOT NULL, first_ns INTEGER NOT NULL, value TEXT NOT NULL, resolved INTEGER NOT NULL DEFAULT 0, latest_sequence INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS active_gaps ON gaps(resolved, sequence);
            CREATE INDEX IF NOT EXISTS gap_identity ON gaps(pane,kind,first_ns);").map_err(|error| return sql_error(&path, error))?;

        let head = load_head(&connection, &path)?.unwrap_or_else(empty_head);

        return Ok(Self {
            connection,
            path,
            journal: directory.join(journal::JOURNAL_NAME),
            head,
        });
    }

    fn rebuild(&mut self, manifest: &TmuxRecordingManifest) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| return sql_error(&self.path, error))?;
        transaction
            .execute_batch(
                "DELETE FROM tracks; DELETE FROM gaps; DELETE FROM events; DELETE FROM head;",
            )
            .map_err(|error| return sql_error(&self.path, error))?;

        let mut head = empty_head();

        let boundary = &manifest.lifecycle_journal;
        visit_records(&self.journal, boundary, |written| {
            return apply_record(&transaction, &self.path, written, &mut head.counts);
        })?;

        head.bytes = boundary.bytes;
        head.sequence = boundary.sequence;
        head.hash.clone_from(&boundary.hash);

        if manifest.catalog != head.counts {
            return Err(Error::invalid(
                "tmux inventory counts differ from the canonical journal",
            ));
        }

        store_head(&transaction, &self.path, &head)?;
        transaction
            .commit()
            .map_err(|error| return sql_error(&self.path, error))?;

        self.head = head;

        return Ok(());
    }

    pub(crate) fn append(
        &mut self,
        records: &[Written],
        boundary: &TmuxJournalBoundary,
    ) -> Result<TmuxCatalogSummary> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| return sql_error(&self.path, error))?;

        let mut counts = self.head.counts.clone();
        for written in records {
            apply_record(&transaction, &self.path, written, &mut counts)?;
        }

        let head = Head {
            bytes: boundary.bytes,
            sequence: boundary.sequence,
            hash: boundary.hash.clone(),
            counts,
        };

        store_head(&transaction, &self.path, &head)?;
        transaction
            .commit()
            .map_err(|error| return sql_error(&self.path, error))?;

        self.head = head;

        return Ok(self.head.counts.clone());
    }

    /// Complete committed inventory counts, independent of the recent snapshot tail.
    pub fn summary(&self) -> &TmuxCatalogSummary {
        return &self.head.counts;
    }

    /// Look up an immutable recording identity and verify its indexed source record.
    ///
    /// # Errors
    /// Returns index or canonical record mismatches.
    pub fn track(&self, id: Uuid) -> Result<Option<TmuxTrackCatalogEntry>> {
        let row = self
            .connection
            .query_row(
                "SELECT sequence,value,completed FROM tracks WHERE id=?1",
                [id.to_string()],
                |row| {
                    return Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ));
                },
            )
            .optional()
            .map_err(|error| return sql_error(&self.path, error))?;

        return row
            .map(|(sequence, value, completed)| {
                return self.decode_track(sequence, &value, completed);
            })
            .transpose();
    }

    /// Read at most 4,096 incarnations, within four MiB, optionally unfinished only.
    ///
    /// # Errors
    /// Returns invalid limits, index failures, or source record mismatches.
    pub fn tracks(
        &self,
        from_sequence: u64,
        limit: u16,
        unfinished_only: bool,
    ) -> Result<Vec<TmuxTrackCatalogEntry>> {
        check_limit(limit)?;

        let mut statement = self.connection.prepare("SELECT sequence,value,completed FROM tracks WHERE sequence>=?1 AND (?2=0 OR completed=0) ORDER BY sequence LIMIT ?3").map_err(|error| return sql_error(&self.path, error))?;
        let rows = statement
            .query_map(params![from_sequence, unfinished_only, limit], |row| {
                return Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ));
            })
            .map_err(|error| return sql_error(&self.path, error))?;

        let mut entries = Vec::new();

        let mut bytes = 0_usize;
        for row in rows {
            let (sequence, value, completed) =
                row.map_err(|error| return sql_error(&self.path, error))?;
            bytes = bytes.saturating_add(value.len());

            if bytes > 4 * 1024 * 1024 {
                return Err(Error::invalid(
                    "tmux track page exceeds four MiB; request fewer entries",
                ));
            }

            entries.push(self.decode_track(sequence, &value, completed)?);
        }

        return Ok(entries);
    }

    /// Read at most 4,096 coverage intervals within four MiB, optionally unresolved only.
    ///
    /// # Errors
    /// Returns invalid limits, index failures, or source record mismatches.
    pub fn gaps(
        &self,
        from_sequence: u64,
        limit: u16,
        unresolved_only: bool,
    ) -> Result<Vec<TmuxGapCatalogEntry>> {
        check_limit(limit)?;

        let mut statement = self.connection.prepare("SELECT sequence,value,latest_sequence FROM gaps WHERE sequence>=?1 AND (?2=0 OR resolved=0) ORDER BY sequence LIMIT ?3").map_err(|error| return sql_error(&self.path, error))?;
        let rows = statement
            .query_map(params![from_sequence, unresolved_only, limit], |row| {
                return Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ));
            })
            .map_err(|error| return sql_error(&self.path, error))?;

        let mut entries = Vec::new();

        let mut bytes = 0_usize;
        for row in rows {
            let (sequence, value, latest) =
                row.map_err(|error| return sql_error(&self.path, error))?;
            bytes = bytes.saturating_add(value.len());

            if bytes > 4 * 1024 * 1024 {
                return Err(Error::invalid(
                    "tmux coverage page exceeds four MiB; request fewer entries",
                ));
            }

            let gap = decode(&self.path, &value)?;
            let (TmuxEvent::CoverageGapOpened { gap: canonical }
            | TmuxEvent::CoverageGapResolved { gap: canonical }) = self.event(latest)?.event
            else {
                return Err(Error::invalid(
                    "tmux gap index points at a non-gap journal record",
                ));
            };
            if canonical != gap {
                return Err(Error::invalid(
                    "tmux gap index differs from its journal record",
                ));
            }

            entries.push(TmuxGapCatalogEntry { sequence, gap });
        }

        return Ok(entries);
    }

    /// Read one indexed lifecycle event, checking its exact canonical bytes and chain hash.
    ///
    /// # Errors
    /// Returns missing events, invalid positions, or source mismatches.
    pub fn event(&self, sequence: u64) -> Result<TmuxEventRecord> {
        let (offset, length, hash) = self
            .connection
            .query_row(
                "SELECT offset,length,hash FROM events WHERE sequence=?1",
                [sequence],
                |row| {
                    return Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                    ));
                },
            )
            .map_err(|error| return sql_error(&self.path, error))?;

        if length > MAX_RECORD_BYTES
            || offset
                .checked_add(length)
                .is_none_or(|end| return end > self.head.bytes)
        {
            return Err(Error::invalid(
                "tmux catalog points outside its committed prefix",
            ));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW | O_NONBLOCK)
            .open(&self.journal)
            .map_err(|error| return Error::filesystem(&self.journal, error))?;

        if !file
            .metadata()
            .map_err(|error| return Error::filesystem(&self.journal, error))?
            .is_file()
        {
            return Err(Error::invalid("tmux journal is not a regular file"));
        }

        file.seek(SeekFrom::Start(offset))
            .map_err(|error| return Error::filesystem(&self.journal, error))?;
        let mut bytes = Vec::new();
        file.take(length)
            .read_to_end(&mut bytes)
            .map_err(|error| return Error::filesystem(&self.journal, error))?;
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|error| return Error::json(&self.journal, error))?;

        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length
            || bytes.last() != Some(&b'\n')
            || record.event.sequence != sequence
            || record.hash != hash
            || record.hash != record_hash(&record.previous_hash, &record.event)?
        {
            return Err(Error::invalid(
                "tmux catalog record differs from canonical evidence",
            ));
        }
        return Ok(record.event);
    }

    fn decode_track(
        &self,
        sequence: u64,
        value: &str,
        completed: bool,
    ) -> Result<TmuxTrackCatalogEntry> {
        let track: TmuxPaneTrack = decode(&self.path, value)?;
        let linked = matches!(
            self.event(sequence)?.event,
            TmuxEvent::PaneTrackLinked { track: canonical } if canonical == track
        );
        if !linked {
            return Err(Error::invalid(
                "tmux track index differs from its canonical link",
            ));
        }
        return Ok(TmuxTrackCatalogEntry {
            sequence,
            track,
            completed,
        });
    }
}

fn apply_record(
    transaction: &Transaction<'_>,
    path: &Path,
    written: &Written,
    counts: &mut TmuxCatalogSummary,
) -> Result<()> {
    let record = &written.record;
    let sequence = record.event.sequence;
    transaction
        .execute(
            "INSERT INTO events VALUES (?1,?2,?3,?4)",
            params![sequence, written.offset, written.length, record.hash],
        )
        .map_err(|error| return sql_error(path, error))?;

    match &record.event.event {
        TmuxEvent::PaneTrackLinked { track } => {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT value FROM tracks WHERE id=?1",
                    [track.session_id.to_string()],
                    |row| return row.get(0),
                )
                .optional()
                .map_err(|error| return sql_error(path, error))?;

            if let Some(existing) = existing {
                let previous: TmuxPaneTrack = decode(path, &existing)?;
                if previous.pane_id != track.pane_id || previous.session != track.session {
                    return Err(Error::invalid("tmux recording identity was rebound"));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO tracks(id,pane,sequence,value) VALUES (?1,?2,?3,?4)",
                        params![
                            track.session_id.to_string(),
                            track.pane_id,
                            sequence,
                            encode(path, track)?
                        ],
                    )
                    .map_err(|error| return sql_error(path, error))?;

                counts.tracks = increment(counts.tracks)?;
                counts.unfinished_tracks = increment(counts.unfinished_tracks)?;
            }
        }
        TmuxEvent::PaneTrackCompleted { session_id, .. } => {
            let changed = transaction
                .execute(
                    "UPDATE tracks SET completed=1 WHERE id=?1 AND completed=0",
                    [session_id.to_string()],
                )
                .map_err(|error| return sql_error(path, error))?;
            counts.unfinished_tracks = counts
                .unfinished_tracks
                .checked_sub(u64::try_from(changed).unwrap_or(u64::MAX))
                .ok_or_else(|| return Error::invalid("tmux unfinished track count underflow"))?;
        }
        TmuxEvent::CoverageGapOpened { gap } => {
            transaction
                .execute(
                    "INSERT INTO gaps VALUES (?1,?2,?3,?4,?5,0,?1)",
                    params![
                        sequence,
                        gap.pane_id,
                        gap.kind.as_str(),
                        gap.first_observed_unix_ns,
                        encode(path, gap)?
                    ],
                )
                .map_err(|error| return sql_error(path, error))?;

            counts.coverage_gaps = increment(counts.coverage_gaps)?;
        }
        TmuxEvent::CoverageGapResolved { gap } => {
            if gap.resolved_unix_ns.is_none() {
                return Err(Error::invalid("resolved tmux gap has no resolution time"));
            }

            let changed = transaction
                .execute(
                    "UPDATE gaps SET value=?1,resolved=1,latest_sequence=?2 WHERE pane IS ?3 AND kind=?4 AND first_ns=?5 AND resolved=0",
                    params![
                        encode(path, gap)?,
                        sequence,
                        gap.pane_id,
                        gap.kind.as_str(),
                        gap.first_observed_unix_ns
                    ],
                )
                .map_err(|error| return sql_error(path, error))?;
            if changed != 1 {
                return Err(Error::invalid(
                    "resolved tmux gap has no unique open interval",
                ));
            }
        }
        _ => {}
    }

    return Ok(());
}

fn checked_index_file(path: &Path, create: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(|error| return Error::filesystem(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| return Error::filesystem(path, error))?;

    if !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(Error::invalid(
            "tmux catalog must be a private file owned by the current user",
        ));
    }
    return Ok(file);
}

fn matches_manifest(head: &Head, manifest: &TmuxRecordingManifest) -> bool {
    let boundary = &manifest.lifecycle_journal;

    return head.bytes == boundary.bytes
        && head.sequence == boundary.sequence
        && head.hash == boundary.hash
        && manifest.catalog == head.counts;
}

fn load_head(connection: &Connection, path: &Path) -> Result<Option<Head>> {
    return connection
        .query_row("SELECT value FROM head WHERE id=1", [], |row| {
            return row.get::<_, String>(0);
        })
        .optional()
        .map_err(|error| return sql_error(path, error))?
        .map(|value| return decode(path, &value))
        .transpose();
}

fn store_head(transaction: &Transaction<'_>, path: &Path, head: &Head) -> Result<()> {
    let value = encode(path, head)?;
    transaction
        .execute("INSERT OR REPLACE INTO head VALUES (1,?1)", [value])
        .map_err(|error| return sql_error(path, error))?;

    return Ok(());
}

fn empty_head() -> Head {
    return Head {
        bytes: 0,
        sequence: 0,
        hash: String::new(),
        counts: TmuxCatalogSummary::default(),
    };
}

fn check_limit(limit: u16) -> Result<()> {
    if limit == 0 || limit > 4096 {
        return Err(Error::invalid(
            "tmux catalog pages require 1 to 4096 entries",
        ));
    }
    return Ok(());
}

fn increment(value: u64) -> Result<u64> {
    return value
        .checked_add(1)
        .ok_or_else(|| return Error::invalid("tmux catalog counter overflow"));
}

fn decode<T: DeserializeOwned>(path: &Path, value: &str) -> Result<T> {
    return serde_json::from_str(value).map_err(|error| return Error::json(path, error));
}

fn encode<T: Serialize>(path: &Path, value: &T) -> Result<String> {
    return serde_json::to_string(value).map_err(|error| return Error::json(path, error));
}

fn sql_error(path: &Path, source: rusqlite::Error) -> Error {
    return Error::Index {
        path: path.to_path_buf(),
        source,
    };
}

#[cfg(test)]
mod tests;
