use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{BufRead as _, BufReader, BufWriter, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use termtrace_model::{
    execution::event::Event,
    execution::request_journal::MutationJournalRecord,
    protocol::control::{
        ControlCommand, ControlData, ControlResponse, ControlResult, MAX_CONTROL_MESSAGE_BYTES,
    },
    terminal::observation::ActorBoundaries,
};
use uuid::Uuid;

use crate::storage::{Error, Result, event_log::EventLogCursor};

pub use termtrace_model::execution::request_journal::{
    MUTATION_JOURNAL_FORMAT, MUTATION_JOURNAL_NAME, MUTATION_JOURNAL_VERSION,
};

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
}

impl MutationJournal {
    /// Create a new empty mutation journal in a session directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal already exists or cannot be durably created.
    pub fn create(session: &Path) -> Result<Self> {
        let path = session.join(MUTATION_JOURNAL_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|source| return Error::filesystem(&path, source))?;
        file.sync_data()
            .map_err(|source| return Error::filesystem(&path, source))?;
        sync_parent_directory(&path)?;
        return Ok(Self {
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
            .is_some_and(|bytes| return bytes <= MAX_MUTATION_JOURNAL_BYTES);
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
        let reader = BufReader::new(
            File::open(&self.path)
                .map_err(|source| return Error::filesystem(&self.path, source))?,
        );
        let mut previous_hash = "0".repeat(64);
        let mut matched = None;
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|source| return Error::filesystem(&self.path, source))?;
            let record = decode_record(&line, index, &previous_hash, &self.path)?;
            previous_hash = record.hash.clone();
            if matched.is_none() && record.request_id == request_id {
                matched = Some(record);
            }
        }
        return Ok(matched);
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
            || self
                .encoded_bytes
                .checked_add(record_bytes)
                .is_none_or(|bytes| return bytes > MAX_MUTATION_JOURNAL_BYTES)
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
        self.encoded_bytes = self.encoded_bytes.saturating_add(record_bytes);
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("mutation journal sequence overflow"))?;
        self.previous_hash = record.hash.clone();
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
        previous_hash = record.hash.clone();
        records.push(record);
    }
    return Ok(records);
}

/// Validate a session's mutation journal against its canonical event log.
///
/// Returns the number of validated journal records.
///
/// # Errors
///
/// Returns an error when journal records or their event boundaries are inconsistent.
pub fn validate_mutation_journal(session: &Path) -> Result<u64> {
    let path = session.join(MUTATION_JOURNAL_NAME);
    let records = read_mutation_journal(&path)?;
    if records.is_empty() {
        return Ok(0);
    }
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    let header_bytes = serde_json::to_vec(cursor.header()).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let header_hash = blake3::hash(&header_bytes).to_hex().to_string();
    let boundaries: HashSet<u64> = records
        .iter()
        .map(|record| return record.event_end_seq)
        .collect();
    let mut hashes = HashMap::new();
    hashes.insert(0, header_hash);
    let mut operations = Vec::new();
    let mut event_count = 0;
    while let Some(event) = cursor.next_record()? {
        event_count = event.seq + 1;
        if let Some(operation_id) = event_operation_id(&event.event) {
            operations.push((event.seq, operation_id));
        }
        if boundaries.contains(&event_count) {
            hashes.insert(event_count, event.hash);
        }
    }
    let _pending = cursor.finish()?;
    let mut request_ids: HashSet<Uuid> = HashSet::new();
    for record in &records {
        if !request_ids.insert(record.request_id) {
            return Err(Error::invalid(
                "mutation journal repeats a request identifier",
            ));
        }
        if record.event_end_seq > event_count {
            return Err(Error::invalid(
                "mutation journal event range exceeds the canonical log",
            ));
        }
        let expected_hash = hashes
            .get(&record.event_end_seq)
            .ok_or_else(|| return Error::invalid("journal event boundary is missing"))?;
        if record.event_hash != *expected_hash {
            return Err(Error::invalid(
                "mutation journal event hash does not match the canonical boundary",
            ));
        }
        if record.event_end_seq > record.event_start_seq {
            let mut actual_operation_ids: Vec<Uuid> = Vec::new();
            let start = operations.partition_point(|(seq, _)| return *seq < record.event_start_seq);
            for operation_id in operations
                .get(start..)
                .unwrap_or_default()
                .iter()
                .take_while(|(seq, _)| return *seq < record.event_end_seq)
                .map(|(_, operation_id)| return *operation_id)
            {
                if actual_operation_ids.last() != Some(&operation_id) {
                    actual_operation_ids.push(operation_id);
                }
            }
            if actual_operation_ids.is_empty() || record.operation_ids != actual_operation_ids {
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
    }
    return Ok(u64::try_from(records.len()).unwrap_or(u64::MAX));
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

fn event_operation_id(event: &Event) -> Option<Uuid> {
    return match event {
        Event::InputIntent { operation_id, .. }
        | Event::InputResult { operation_id, .. }
        | Event::ResizeIntent { operation_id, .. }
        | Event::ResizeResult { operation_id, .. }
        | Event::SignalIntent { operation_id, .. }
        | Event::SignalResult { operation_id, .. }
        | Event::Marker { operation_id, .. } => Some(*operation_id),
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

fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| return Error::invalid("journal path has no parent"))?;
    File::open(parent)
        .and_then(|directory| return directory.sync_all())
        .map_err(|source| return Error::filesystem(parent, source))?;
    return Ok(());
}

#[cfg(test)]
mod tests {
    use std::fs;

    use termtrace_model::protocol::control::CONTROL_PROTOCOL_VERSION;

    use super::*;

    fn error_response(request_id: Uuid, message: String) -> ControlResponse {
        return ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "fixture".to_owned(),
                message,
                details: None,
            },
        };
    }

    #[test]
    fn incompatible_version_is_rejected_before_response_decode() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join(MUTATION_JOURNAL_NAME);
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
            Error::InvalidEvidence { reason }
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
            Error::InvalidEvidence { reason }
                if reason == "mutation journal byte budget would be exceeded"
        ));
        assert_eq!(
            fs::metadata(temporary.path().join(MUTATION_JOURNAL_NAME))
                .expect("journal metadata")
                .len(),
            0
        );
    }
}
