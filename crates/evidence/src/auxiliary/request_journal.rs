use crate::storage::{
    artifact::sync_parent_directory,
    event_log::EventLogCursor,
    identifier_index::{index_error, IdentifierSet, RequestIndex, RequestPosition},
    Error, Result,
};
use rusqlite::{params, Connection, OptionalExtension as _};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{
        BufRead as _, BufReader, BufWriter, ErrorKind, Read as _, Seek as _, SeekFrom, Write as _,
    },
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    str,
};
pub use termtrace_model::execution::request_journal::{
    MUTATION_JOURNAL_FORMAT, MUTATION_JOURNAL_VERSION,
};
use termtrace_model::{
    execution::event::{Event, InputSource},
    execution::layout::SessionLayout,
    execution::request_journal::MutationJournalRecord,
    protocol::control::{
        ControlCommand, ControlData, ControlResponse, ControlResult, MAX_CONTROL_MESSAGE_BYTES,
    },
    terminal::observation::ActorBoundaries,
};
use uuid::Uuid;

const MAX_MUTATION_JOURNAL_BYTES: u64 = 64 * MAX_CONTROL_MESSAGE_BYTES;
const MAX_MUTATION_JOURNAL_RECORD_BYTES: u64 = 3 * MAX_CONTROL_MESSAGE_BYTES;

#[derive(Deserialize)]
struct MutationJournalHeader {
    format: String,
    version: u32,
}

#[derive(Serialize)]
struct MutationJournalBody<'record> {
    format: &'record str,
    version: u32,
    seq: u64,
    prev_hash: &'record str,
    request_id: Uuid,
    command_hash: &'record str,
    operation_ids: &'record [Uuid],
    event_start_seq: u64,
    event_end_seq: u64,
    event_hash: &'record str,
    response: &'record ControlResponse,
}

/// Append-only, hash-chained journal of completed mutation requests.
#[derive(Debug)]
pub struct MutationJournal {
    path: PathBuf,
    writer: BufWriter<File>,
    next_seq: u64,
    encoded_bytes: u64,
    previous_hash: String,
    index: RequestIndex,
}

impl MutationJournal {
    /// Create a new empty mutation journal in a session directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal already exists or cannot be durably created.
    pub fn create(session: &Path) -> Result<Self> {
        let path = SessionLayout::new(session).mutation_journal();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|source| return Error::filesystem(&path, source))?;
        file.sync_data()
            .map_err(|source| return Error::filesystem(&path, source))?;

        sync_parent_directory(&path)?;

        let index = RequestIndex::create(&session.join("mutation-index.sqlite3"))?;

        return Ok(Self {
            index,
            path,
            writer: BufWriter::new(file),
            next_seq: 0,
            encoded_bytes: 0,
            previous_hash: "0".repeat(64),
        });
    }

    /// Number of records durably appended to the journal.
    #[must_use]
    pub fn len(&self) -> u64 {
        return self.next_seq;
    }

    /// Whether the journal contains no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        return self.next_seq == 0;
    }

    /// Whether the journal can accept another maximum-sized record.
    #[must_use]
    pub fn has_record_capacity(&self) -> bool {
        return self
            .encoded_bytes
            .checked_add(MAX_MUTATION_JOURNAL_RECORD_BYTES)
            .is_some();
    }

    /// Atomically reserve operation identifiers for the lifetime of this recording.
    ///
    /// # Errors
    /// Returns an error if the disk index cannot commit the reservation.
    pub fn reserve_operations(&mut self, identifiers: &[Uuid]) -> Result<bool> {
        return self.index.reserve(identifiers);
    }

    /// Find a completed request by its correlation identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when pending data cannot be flushed or the journal is invalid.
    pub fn lookup(&mut self, request_id: Uuid) -> Result<Option<MutationJournalRecord>> {
        self.writer
            .flush()
            .map_err(|source| return Error::filesystem(&self.path, source))?;

        let Some(position) = self.index.lookup(request_id)? else {
            return Ok(None);
        };
        if position.length > MAX_MUTATION_JOURNAL_RECORD_BYTES {
            return Err(Error::invalid("mutation index record length exceeds limit"));
        }

        let mut file = File::open(&self.path)
            .map_err(|source| return Error::filesystem(&self.path, source))?;
        file.seek(SeekFrom::Start(position.offset))
            .map_err(|source| return Error::filesystem(&self.path, source))?;
        let mut line = String::new();
        file.take(position.length)
            .read_to_string(&mut line)
            .map_err(|source| return Error::filesystem(&self.path, source))?;

        let sequence = usize::try_from(position.sequence).map_err(|source| {
            return Error::invalid_with_source("mutation sequence exceeds platform range", source);
        })?;

        let record = decode_record(
            line.trim_end_matches('\n'),
            sequence,
            &position.previous_hash,
            &self.path,
        )?;

        if record.request_id != request_id
            || record.hash != position.hash
            || u64::try_from(line.len()).unwrap_or(u64::MAX) != position.length
        {
            return Err(Error::invalid(
                "mutation journal does not match its committed index",
            ));
        }
        return Ok(Some(record));
    }

    /// Durably append a completed request and its canonical event boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is inconsistent, exceeds the journal budget, or cannot
    /// be persisted.
    #[allow(
        clippy::too_many_arguments,
        reason = "the durable record contract names each canonical boundary explicitly"
    )]
    pub fn append(
        &mut self,
        request_id: Uuid,
        command_hash: String,
        operation_ids: Vec<Uuid>,
        event_start_seq: u64,
        event_end_seq: u64,
        event_hash: String,
        response: ControlResponse,
    ) -> Result<MutationJournalRecord> {
        if event_end_seq < event_start_seq {
            return Err(Error::invalid("mutation event range is reversed"));
        }

        if response.request_id != request_id {
            return Err(Error::invalid(
                "mutation response request ID does not match journal request ID",
            ));
        }

        let body = MutationJournalBody {
            format: MUTATION_JOURNAL_FORMAT,
            version: MUTATION_JOURNAL_VERSION,
            seq: self.next_seq,
            prev_hash: &self.previous_hash,
            request_id,
            command_hash: &command_hash,
            operation_ids: &operation_ids,
            event_start_seq,
            event_end_seq,
            event_hash: &event_hash,
            response: &response,
        };
        let hash = hash_body(&body, &self.path)?;
        let record = MutationJournalRecord {
            format: MUTATION_JOURNAL_FORMAT.to_owned(),
            version: MUTATION_JOURNAL_VERSION,
            seq: self.next_seq,
            prev_hash: self.previous_hash.clone(),
            hash,
            request_id,
            command_hash,
            operation_ids,
            event_start_seq,
            event_end_seq,
            event_hash,
            response,
        };
        let mut encoded = serde_json::to_vec(&record).map_err(|source| {
            return Error::InvalidJson {
                path: self.path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        encoded.push(b'\n');
        let record_bytes = u64::try_from(encoded.len()).unwrap_or(u64::MAX);

        if record_bytes > MAX_MUTATION_JOURNAL_RECORD_BYTES
            || self.encoded_bytes.checked_add(record_bytes).is_none()
        {
            return Err(Error::invalid(
                "mutation journal byte budget would be exceeded",
            ));
        }

        self.writer
            .write_all(&encoded)
            .and_then(|()| return self.writer.flush())
            .and_then(|()| return self.writer.get_ref().sync_data())
            .map_err(|source| return Error::filesystem(&self.path, source))?;

        self.index.insert(
            request_id,
            &RequestPosition {
                offset: self.encoded_bytes,
                length: record_bytes,
                sequence: record.seq,
                previous_hash: record.prev_hash.clone(),
                hash: record.hash.clone(),
            },
        )?;

        self.encoded_bytes = self.encoded_bytes.saturating_add(record_bytes);
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("mutation journal sequence overflow"))?;

        self.previous_hash.clone_from(&record.hash);

        return Ok(record);
    }
}

#[derive(Serialize)]
struct GuardedCommand<'command> {
    command: &'command ControlCommand,
    expected_presented_state_hash: Option<&'command str>,
}

/// Hash a control command together with its optimistic state guard.
///
/// # Errors
///
/// Returns an error when the guarded command cannot be encoded as canonical JSON.
pub fn guarded_command_hash(
    command: &ControlCommand,
    expected_presented_state_hash: Option<&str>,
) -> Result<String> {
    let guarded = GuardedCommand {
        command,
        expected_presented_state_hash,
    };
    let bytes = serde_json::to_vec(&guarded).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<guarded-control-command>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

/// Read and validate every record in a mutation journal.
///
/// A missing journal is treated as an empty journal.
///
/// # Errors
///
/// Returns an error when the path is not a bounded regular file or any record is invalid.
pub fn read_mutation_journal(path: &Path) -> Result<Vec<MutationJournalRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let metadata =
        fs_err::symlink_metadata(path).map_err(|source| return Error::filesystem(path, source))?;

    if !metadata.is_file() || metadata.len() > MAX_MUTATION_JOURNAL_BYTES {
        return Err(Error::invalid(
            "mutation journal is not a bounded regular file",
        ));
    }

    let reader =
        BufReader::new(File::open(path).map_err(|source| return Error::filesystem(path, source))?);

    let mut records: Vec<MutationJournalRecord> = Vec::new();
    let mut previous_hash = "0".repeat(64);

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|source| return Error::filesystem(path, source))?;
        if u64::try_from(line.len()).unwrap_or(u64::MAX) > MAX_MUTATION_JOURNAL_RECORD_BYTES {
            return Err(Error::invalid(
                "mutation journal record exceeds its byte limit",
            ));
        }

        let record = decode_record(&line, index, &previous_hash, path)?;
        previous_hash.clone_from(&record.hash);
        records.push(record);
    }

    return Ok(records);
}

/// Streaming journal reader with a per-record bound and no lifetime allocation limit.
pub struct MutationJournalCursor {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    sequence: usize,
    previous_hash: String,
}

impl MutationJournalCursor {
    /// Open a journal; an absent journal is an empty stream.
    ///
    /// # Errors
    /// Returns an error for an inaccessible or nonregular journal.
    pub fn open(path: &Path) -> Result<Self> {
        let reader = match fs_err::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Some(BufReader::new(
                File::open(path).map_err(|source| return Error::filesystem(path, source))?,
            )),
            Ok(_) => return Err(Error::invalid("mutation journal must be a regular file")),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(source) => return Err(Error::filesystem(path, source)),
        };

        return Ok(Self {
            path: path.to_path_buf(),
            reader,
            sequence: 0,
            previous_hash: "0".repeat(64),
        });
    }

    /// Read and hash-validate one complete record.
    ///
    /// # Errors
    /// Returns an error for malformed, truncated, or oversized records.
    pub fn next_record(&mut self) -> Result<Option<MutationJournalRecord>> {
        let Some(reader) = self.reader.as_mut() else {
            return Ok(None);
        };

        let mut line = Vec::new();
        let bytes = reader
            .by_ref()
            .take(MAX_MUTATION_JOURNAL_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|source| return Error::filesystem(&self.path, source))?;

        if bytes == 0 {
            return Ok(None);
        }

        if u64::try_from(bytes).unwrap_or(u64::MAX) > MAX_MUTATION_JOURNAL_RECORD_BYTES
            || line.last() != Some(&b'\n')
        {
            return Err(Error::invalid(
                "mutation journal record is oversized or unterminated",
            ));
        }

        let line = str::from_utf8(&line).map_err(|source| {
            return Error::invalid_with_source("mutation journal is not UTF-8", source);
        })?;

        let record = decode_record(line, self.sequence, &self.previous_hash, &self.path)?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("mutation journal sequence overflow"))?;

        self.previous_hash.clone_from(&record.hash);

        return Ok(Some(record));
    }
}

/// Validate a session's mutation journal against its canonical event log.
///
/// Returns the number of validated journal records.
///
/// # Errors
///
/// Returns an error when journal records or their event boundaries are inconsistent.
pub fn validate_mutation_journal(session: &Path) -> Result<u64> {
    let path = SessionLayout::new(session).mutation_journal();
    let mut journal = MutationJournalCursor::open(&path)?;
    let database = Connection::open("").map_err(index_error)?;
    database
        .execute_batch(
            "PRAGMA cache_size=-1024; PRAGMA journal_mode=OFF; PRAGMA temp_store=FILE;
        CREATE TABLE boundaries (seq INTEGER PRIMARY KEY, hash TEXT);
        CREATE TABLE operations (seq INTEGER PRIMARY KEY, id BLOB NOT NULL, is_host INTEGER NOT NULL);",
        )
        .map_err(index_error)?;

    let mut identifiers = IdentifierSet::default();
    let mut count = 0_u64;
    let mut final_hash = String::new();

    while let Some(record) = journal.next_record()? {
        if !identifiers.insert(record.request_id)? {
            return Err(Error::invalid(
                "mutation journal repeats a request identifier",
            ));
        }

        database
            .execute(
                "INSERT OR IGNORE INTO boundaries (seq) VALUES (?1)",
                [record.event_end_seq],
            )
            .map_err(index_error)?;

        count += 1;
        final_hash = record.hash;
    }

    if count == 0 {
        return Ok(0);
    }

    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let header_bytes = serde_json::to_vec(cursor.header()).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let header_hash = blake3::hash(&header_bytes).to_hex().to_string();
    database
        .execute("UPDATE boundaries SET hash=?1 WHERE seq=0", [&header_hash])
        .map_err(index_error)?;

    {
        let mut boundary = database
            .prepare("UPDATE boundaries SET hash=?1 WHERE seq=?2")
            .map_err(index_error)?;
        let mut operation = database
            .prepare("INSERT INTO operations VALUES (?1, ?2, ?3)")
            .map_err(index_error)?;

        let mut host_operations = HashSet::new();

        while let Some(event) = cursor.next_record()? {
            boundary
                .execute(params![event.hash, event.seq + 1])
                .map_err(index_error)?;

            if let Some((id, is_host)) = event_operation(&event.event, &mut host_operations) {
                operation
                    .execute(params![
                        event.seq,
                        id.as_bytes().as_slice(),
                        if is_host { 1 } else { 0 },
                    ])
                    .map_err(index_error)?;
            }
        }
    }

    let _pending = cursor.finish()?;
    let mut journal = MutationJournalCursor::open(&path)?;
    let mut verified_count = 0;
    let mut verified_hash = String::new();

    while let Some(record) = journal.next_record()? {
        let expected_hash: Option<String> = database
            .query_row(
                "SELECT hash FROM boundaries WHERE seq=?1",
                [record.event_end_seq],
                |row| return row.get(0),
            )
            .optional()
            .map_err(index_error)?
            .flatten();
        if expected_hash.as_ref() != Some(&record.event_hash) {
            return Err(Error::invalid(
                "mutation journal event hash does not match the canonical boundary",
            ));
        }

        if record.event_end_seq > record.event_start_seq {
            let mut statement = database
                .prepare(
                    "SELECT id FROM operations WHERE seq>=?1 AND seq<?2 AND is_host=0 ORDER BY seq",
                )
                .map_err(index_error)?;
            let mut rows = statement
                .query([record.event_start_seq, record.event_end_seq])
                .map_err(index_error)?;

            let mut previous = None;

            let mut matched_count = 0;
            while let Some(row) = rows.next().map_err(index_error)? {
                let bytes: Vec<u8> = row.get(0).map_err(index_error)?;
                let id = Uuid::from_slice(&bytes).map_err(|source| {
                    return Error::invalid_with_source(
                        "canonical operation identifier index is invalid",
                        source,
                    );
                })?;
                if previous == Some(id) {
                    continue;
                }

                previous = Some(id);

                if record.operation_ids.get(matched_count) == Some(&id) {
                    matched_count += 1;
                }
            }

            if matched_count != record.operation_ids.len() {
                return Err(Error::invalid(
                    "mutation journal operation IDs do not match its canonical range",
                ));
            }
        } else if !record.operation_ids.is_empty() {
            return Err(Error::invalid(
                "event-free mutation journal outcome names canonical operations",
            ));
        }

        if let Some(boundaries) = response_boundaries(&record.response)
            && (boundaries.observed.next_event_seq != record.event_end_seq
                || boundaries.observed.event_hash != record.event_hash)
        {
            return Err(Error::invalid(
                "mutation response boundary does not match its canonical range",
            ));
        }

        verified_count += 1;
        verified_hash = record.hash;
    }

    if verified_count != count || verified_hash != final_hash {
        return Err(Error::invalid(
            "mutation journal changed during verification",
        ));
    }
    return Ok(count);
}

fn response_boundaries(response: &ControlResponse) -> Option<&ActorBoundaries> {
    return match &response.result {
        ControlResult::Ok { data } => match data {
            ControlData::Acknowledged { boundaries }
            | ControlData::InputApplied { boundaries, .. }
            | ControlData::InputSequenceApplied { boundaries, .. }
            | ControlData::ActionApplied { boundaries, .. }
            | ControlData::InputNoop { boundaries, .. }
            | ControlData::Activated { boundaries, .. }
            | ControlData::Snapshot { boundaries, .. }
            | ControlData::Status { boundaries, .. }
            | ControlData::Marked { boundaries, .. } => Some(boundaries),
            ControlData::ScreenFrame { .. }
            | ControlData::EventCursor { .. }
            | ControlData::StableSnapshot { .. }
            | ControlData::ScreenWait { .. } => None,
        },
        ControlResult::Error {
            details: Some(details),
            ..
        } => Some(&details.boundaries),
        ControlResult::Error { details: None, .. } => None,
    };
}

fn event_operation(event: &Event, host_operations: &mut HashSet<Uuid>) -> Option<(Uuid, bool)> {
    return match event {
        Event::InputIntent {
            operation_id,
            source,
            ..
        } => {
            let is_host = *source == InputSource::Host;
            if is_host {
                host_operations.insert(*operation_id);
            }

            Some((*operation_id, is_host))
        }
        Event::InputResult { operation_id, .. } => {
            let is_host = host_operations.contains(operation_id);

            Some((*operation_id, is_host))
        }
        Event::ResizeIntent { operation_id, .. }
        | Event::ResizeResult { operation_id, .. }
        | Event::SignalIntent { operation_id, .. }
        | Event::SignalResult { operation_id, .. }
        | Event::Marker { operation_id, .. }
        | Event::Application { operation_id, .. } => Some((*operation_id, false)),
        _ => None,
    };
}

fn decode_record(
    line: &str,
    index: usize,
    previous_hash: &str,
    path: &Path,
) -> Result<MutationJournalRecord> {
    let header: MutationJournalHeader = serde_json::from_str(line).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if header.format != MUTATION_JOURNAL_FORMAT || header.version != MUTATION_JOURNAL_VERSION {
        return Err(Error::invalid(format!(
            "unsupported mutation journal format `{}` version {}",
            header.format, header.version
        )));
    }

    let record: MutationJournalRecord = serde_json::from_str(line).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let seq = u64::try_from(index).unwrap_or(u64::MAX);
    if record.seq != seq
        || record.prev_hash != previous_hash
        || record.response.request_id != record.request_id
        || record.event_end_seq < record.event_start_seq
    {
        return Err(Error::invalid(
            "mutation journal sequence, correlation, or event range is invalid",
        ));
    }

    let body = MutationJournalBody {
        format: &record.format,
        version: record.version,
        seq: record.seq,
        prev_hash: &record.prev_hash,
        request_id: record.request_id,
        command_hash: &record.command_hash,
        operation_ids: &record.operation_ids,
        event_start_seq: record.event_start_seq,
        event_end_seq: record.event_end_seq,
        event_hash: &record.event_hash,
        response: &record.response,
    };

    if record.hash != hash_body(&body, path)? {
        return Err(Error::invalid(
            "mutation journal record hash does not match",
        ));
    }
    return Ok(record);
}

fn hash_body(body: &MutationJournalBody<'_>, path: &Path) -> Result<String> {
    let bytes = serde_json::to_vec(body).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::event_log::{EventLogWriter, SessionClock};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt as _;
    use termtrace_model::{
        execution::event::{
            Event, EventLogHeader, InputDescriptor, InputKind, InputSource, OperationStatus,
        },
        protocol::{control::CONTROL_PROTOCOL_VERSION, mutation::MutationEffect},
        value::ByteString,
    };

    fn error_response(request_id: Uuid, message: String) -> ControlResponse {
        return ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "fixture".to_owned(),
                message,
                effect: MutationEffect::NotApplied,
                details: None,
            },
        };
    }

    #[test]
    fn incompatible_version_is_rejected_before_response_decode() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = SessionLayout::new(temporary.path()).mutation_journal();
        fs::write(
            &path,
            format!(
                "{{\"format\":\"{MUTATION_JOURNAL_FORMAT}\",\"version\":1,\"response\":{{\"old\":\"shape\"}}}}\n"
            ),
        )
        .expect("old journal fixture");

        let error = read_mutation_journal(&path).expect_err("old version must be rejected");
        assert!(matches!(
            error,
            Error::InvalidEvidence { reason, .. }
                if reason.contains("unsupported mutation journal format")
                    && reason.contains("version 1")
        ));
    }

    #[test]
    fn lookup_streams_valid_records_and_preserves_exact_response() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let mut journal = MutationJournal::create(temporary.path()).expect("journal");
        let first_id = Uuid::now_v7();
        let second_id = Uuid::now_v7();
        let first_response = error_response(first_id, "first".to_owned());
        journal
            .append(
                first_id,
                "a".repeat(64),
                Vec::new(),
                0,
                0,
                "b".repeat(64),
                first_response.clone(),
            )
            .expect("first record");
        journal
            .append(
                second_id,
                "c".repeat(64),
                Vec::new(),
                0,
                0,
                "d".repeat(64),
                error_response(second_id, "second".to_owned()),
            )
            .expect("second record");
        let found = journal
            .lookup(first_id)
            .expect("lookup")
            .expect("first record");
        assert_eq!(found.response, first_response);
    }

    #[test]
    fn oversized_record_is_not_persisted() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let mut journal = MutationJournal::create(temporary.path()).expect("journal");
        let request_id = Uuid::now_v7();
        let oversized =
            "x".repeat(usize::try_from(MAX_MUTATION_JOURNAL_RECORD_BYTES).expect("record limit"));
        let error = journal
            .append(
                request_id,
                "a".repeat(64),
                Vec::new(),
                0,
                0,
                "b".repeat(64),
                error_response(request_id, oversized),
            )
            .expect_err("oversized record");
        assert!(matches!(
            error,
            Error::InvalidEvidence { reason, .. }
                if reason == "mutation journal byte budget would be exceeded"
        ));
        assert_eq!(
            fs::metadata(SessionLayout::new(temporary.path()).mutation_journal())
                .expect("journal metadata")
                .len(),
            0
        );
    }

    fn secure_directory(path: &Path) {
        fs::create_dir_all(path).expect("create secure directory");
        fs::set_permissions(path, Permissions::from_mode(0o700))
            .expect("secure directory permissions");
    }

    fn input_intent_event(operation_id: Uuid, source: InputSource) -> Event {
        return Event::InputIntent {
            operation_id,
            source,
            input_kind: InputKind::Text,
            descriptor: InputDescriptor::Text,
            data: ByteString::new(vec![b'a']),
            terminal_modes_hash: None,
            semantic_input: None,
            caller_intent: None,
        };
    }

    fn input_result_event(operation_id: Uuid) -> Event {
        return Event::InputResult {
            operation_id,
            status: OperationStatus::Applied,
            bytes_written: 1,
            error_code: None,
        };
    }

    #[test]
    fn validate_mutation_journal_succeeds_with_interleaved_host_query_response() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("session");
        secure_directory(&session);
        let session = session.as_path();
        let clock = SessionClock::now().expect("clock");
        let header = EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64).as_str(),
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm",
        );

        let mut writer =
            EventLogWriter::create(SessionLayout::new(session).event_log(), &header, clock)
                .expect("event log");

        let client_op1 = Uuid::now_v7();
        let host_op = Uuid::now_v7();
        let client_op2 = Uuid::now_v7();

        writer
            .append(input_intent_event(client_op1, InputSource::Agent))
            .expect("append");
        writer
            .append(input_result_event(client_op1))
            .expect("append");
        // Interleaved host query response
        writer
            .append(input_intent_event(host_op, InputSource::Host))
            .expect("append");
        writer.append(input_result_event(host_op)).expect("append");
        writer
            .append(input_intent_event(client_op2, InputSource::Agent))
            .expect("append");
        writer
            .append(input_result_event(client_op2))
            .expect("append");
        let final_event_hash = writer.current_hash();
        let end_seq = writer.next_seq();
        drop(writer);

        let mut journal = MutationJournal::create(session).expect("journal");
        let request_id = Uuid::now_v7();
        journal
            .append(
                request_id,
                "command_hash".to_owned(),
                vec![client_op1, client_op2],
                0,
                end_seq,
                final_event_hash,
                error_response(request_id, "ok".to_owned()),
            )
            .expect("append journal");
        drop(journal);

        let verified = validate_mutation_journal(session).expect("validation should succeed");
        assert_eq!(verified, 1);
    }

    #[test]
    fn validate_mutation_journal_rejects_host_query_response_attributed_to_client() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("session");
        secure_directory(&session);
        let session = session.as_path();
        let clock = SessionClock::now().expect("clock");
        let header = EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64).as_str(),
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm",
        );

        let mut writer =
            EventLogWriter::create(SessionLayout::new(session).event_log(), &header, clock)
                .expect("event log");

        let host_op = Uuid::now_v7();
        writer
            .append(input_intent_event(host_op, InputSource::Host))
            .expect("append");
        writer.append(input_result_event(host_op)).expect("append");
        let final_event_hash = writer.current_hash();
        let end_seq = writer.next_seq();
        drop(writer);

        let mut journal = MutationJournal::create(session).expect("journal");
        let request_id = Uuid::now_v7();
        // Client tries to claim the host operation ID
        journal
            .append(
                request_id,
                "command_hash".to_owned(),
                vec![host_op],
                0,
                end_seq,
                final_event_hash,
                error_response(request_id, "ok".to_owned()),
            )
            .expect("append journal");
        drop(journal);

        let error = validate_mutation_journal(session).expect_err("host op must be rejected");
        assert!(matches!(
            error,
            Error::InvalidEvidence { reason, .. }
                if reason == "mutation journal operation IDs do not match its canonical range"
        ));
    }

    #[test]
    fn validate_mutation_journal_succeeds_when_operation_ids_is_empty_and_host_operations_interleaved(
    ) {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("session");
        secure_directory(&session);
        let session = session.as_path();
        let clock = SessionClock::now().expect("clock");
        let header = EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64).as_str(),
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm",
        );

        let mut writer =
            EventLogWriter::create(SessionLayout::new(session).event_log(), &header, clock)
                .expect("event log");

        let host_op = Uuid::now_v7();
        writer
            .append(input_intent_event(host_op, InputSource::Host))
            .expect("append");
        writer.append(input_result_event(host_op)).expect("append");
        let final_event_hash = writer.current_hash();
        let end_seq = writer.next_seq();
        drop(writer);

        let mut journal = MutationJournal::create(session).expect("journal");
        let request_id = Uuid::now_v7();
        // Empty operation_ids with end_seq > start_seq
        journal
            .append(
                request_id,
                "command_hash".to_owned(),
                Vec::new(),
                0,
                end_seq,
                final_event_hash,
                error_response(request_id, "ok".to_owned()),
            )
            .expect("append journal");
        drop(journal);

        let verified = validate_mutation_journal(session).expect("validation should succeed");
        assert_eq!(verified, 1);
    }

    #[test]
    fn validate_mutation_journal_rejects_out_of_order_operations() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("session");
        secure_directory(&session);
        let session = session.as_path();
        let clock = SessionClock::now().expect("clock");
        let header = EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64).as_str(),
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm",
        );

        let mut writer =
            EventLogWriter::create(SessionLayout::new(session).event_log(), &header, clock)
                .expect("event log");

        let client_op1 = Uuid::now_v7();
        let client_op2 = Uuid::now_v7();

        writer
            .append(input_intent_event(client_op1, InputSource::Agent))
            .expect("append");
        writer
            .append(input_result_event(client_op1))
            .expect("append");
        writer
            .append(input_intent_event(client_op2, InputSource::Agent))
            .expect("append");
        writer
            .append(input_result_event(client_op2))
            .expect("append");
        let final_event_hash = writer.current_hash();
        let end_seq = writer.next_seq();
        drop(writer);

        let mut journal = MutationJournal::create(session).expect("journal");
        let request_id = Uuid::now_v7();
        // Reversed order
        journal
            .append(
                request_id,
                "command_hash".to_owned(),
                vec![client_op2, client_op1],
                0,
                end_seq,
                final_event_hash,
                error_response(request_id, "ok".to_owned()),
            )
            .expect("append journal");
        drop(journal);

        let error = validate_mutation_journal(session).expect_err("out of order must fail");
        assert!(matches!(
            error,
            Error::InvalidEvidence { reason, .. }
                if reason == "mutation journal operation IDs do not match its canonical range"
        ));
    }

    #[test]
    fn validate_mutation_journal_succeeds_with_partial_input_sequence_and_interleaving() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("session");
        secure_directory(&session);
        let session = session.as_path();
        let clock = SessionClock::now().expect("clock");
        let header = EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64).as_str(),
            clock.started_unix_ns(),
            80,
            24,
            720,
            432,
            "xterm",
        );

        let mut writer =
            EventLogWriter::create(SessionLayout::new(session).event_log(), &header, clock)
                .expect("event log");

        let atom1 = Uuid::now_v7();
        let host_op = Uuid::now_v7();
        let atom2 = Uuid::now_v7();

        // Atom 1 applied
        writer
            .append(input_intent_event(atom1, InputSource::Agent))
            .expect("append");
        writer.append(input_result_event(atom1)).expect("append");
        // Host query response interleaved
        writer
            .append(input_intent_event(host_op, InputSource::Host))
            .expect("append");
        writer.append(input_result_event(host_op)).expect("append");
        // Atom 2 attempted and failed during write
        writer
            .append(input_intent_event(atom2, InputSource::Agent))
            .expect("append");
        writer
            .append(Event::InputResult {
                operation_id: atom2,
                status: OperationStatus::Failed,
                bytes_written: 0,
                error_code: Some("io_broken_pipe".to_owned()),
            })
            .expect("append");
        let final_event_hash = writer.current_hash();
        let end_seq = writer.next_seq();
        drop(writer);

        let mut journal = MutationJournal::create(session).expect("journal");
        let request_id = Uuid::now_v7();
        // Only atom1 was applied, so operation_ids has [atom1]
        journal
            .append(
                request_id,
                "command_hash".to_owned(),
                vec![atom1],
                0,
                end_seq,
                final_event_hash,
                error_response(request_id, "partial failure".to_owned()),
            )
            .expect("append journal");
        drop(journal);

        let verified = validate_mutation_journal(session).expect("validation should succeed");
        assert_eq!(verified, 1);
    }
}
