//! Persistent bounded-memory mutation deduplication acceleration.

use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{BufRead as _, BufReader, BufWriter, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::sync_parent_directory,
    control::{ControlCommand, ControlData, ControlResponse, ControlResult},
    event::Event,
    event_log::EventLog,
};

/// Relative persistent mutation journal path in a session.
pub const MUTATION_JOURNAL_NAME: &str = "requests.jsonl";
/// Mutation journal record format identifier.
pub const MUTATION_JOURNAL_FORMAT: &str = "termtrace-mutation-request";
/// Mutation journal schema version.
pub const MUTATION_JOURNAL_VERSION: u32 = 1;

/// One durable mutation request outcome tied to a canonical event boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationJournalRecord {
    /// Stable record format identifier.
    pub format: String,
    /// Record schema version.
    pub version: u32,
    /// Contiguous zero-based journal sequence.
    pub seq: u64,
    /// Prior journal record hash, or 64 zeroes for the first record.
    pub prev_hash: String,
    /// Hash of this record body.
    pub hash: String,
    /// Caller-owned mutation request identifier.
    pub request_id: Uuid,
    /// BLAKE3 of the normalized serialized control command.
    pub command_hash: String,
    /// Canonical operation identifiers created by the request.
    pub operation_ids: Vec<Uuid>,
    /// Inclusive next-event boundary before command execution.
    pub event_start_seq: u64,
    /// Exclusive next-event boundary after command execution.
    pub event_end_seq: u64,
    /// Canonical event hash at `event_end_seq`.
    pub event_hash: String,
    /// Exact actor response returned for safe retry.
    pub response: ControlResponse,
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

/// Append-only writer for persistent mutation request outcomes.
#[derive(Debug)]
pub(crate) struct MutationJournal {
    path: PathBuf,
    writer: BufWriter<File>,
    next_seq: u64,
    previous_hash: String,
}

impl MutationJournal {
    /// Create a new owner-only mutation journal.
    pub(crate) fn create(session: &Path) -> Result<Self> {
        let path = session.join(MUTATION_JOURNAL_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
        file.sync_data().map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        sync_parent_directory(&path)?;
        return Ok(Self {
            path,
            writer: BufWriter::new(file),
            next_seq: 0,
            previous_hash: "0".repeat(64),
        });
    }

    /// Number of durably journaled mutation requests.
    pub(crate) fn len(&self) -> u64 {
        return self.next_seq;
    }

    /// Find a prior request outcome, validating the journal chain first.
    pub(crate) fn lookup(&mut self, request_id: Uuid) -> Result<Option<MutationJournalRecord>> {
        self.writer.flush().map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        return Ok(read_mutation_journal(&self.path)?
            .into_iter()
            .find(|record| return record.request_id == request_id));
    }

    /// Durably append a final response before it is sent to the caller.
    pub(crate) fn append(
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
            return Err(invalid_journal("mutation event range is reversed"));
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
        let mut encoded = serde_json::to_vec(&record)
            .map_err(|source| return json_error(self.path.clone(), source))?;
        encoded.push(b'\n');
        self.writer
            .write_all(&encoded)
            .and_then(|()| return self.writer.flush())
            .and_then(|()| return self.writer.get_ref().sync_data())
            .map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;
        self.next_seq = self.next_seq.checked_add(1).ok_or_else(|| {
            return invalid_journal("mutation journal sequence overflow");
        })?;
        self.previous_hash = record.hash.clone();
        return Ok(record);
    }
}

/// Hash the normalized command used for request-ID collision detection.
pub(crate) fn command_hash(command: &ControlCommand) -> Result<String> {
    let bytes = serde_json::to_vec(command)
        .map_err(|source| return json_error(PathBuf::from("<control-command>"), source))?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

/// Read and validate a complete persistent mutation journal.
pub fn read_mutation_journal(path: &Path) -> Result<Vec<MutationJournalRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = BufReader::new(File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?);
    let mut records = Vec::new();
    let mut previous_hash = "0".repeat(64);
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        let record: MutationJournalRecord = serde_json::from_str(&line)
            .map_err(|source| return json_error(path.to_path_buf(), source))?;
        let seq = u64::try_from(index).unwrap_or(u64::MAX);
        if record.format != MUTATION_JOURNAL_FORMAT
            || record.version != MUTATION_JOURNAL_VERSION
            || record.seq != seq
            || record.prev_hash != previous_hash
            || record.response.request_id != record.request_id
            || record.event_end_seq < record.event_start_seq
        {
            return Err(invalid_journal(
                "mutation journal format, sequence, correlation, or event range is invalid",
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
            return Err(invalid_journal(
                "mutation journal record hash does not match",
            ));
        }
        previous_hash = record.hash.clone();
        records.push(record);
    }
    return Ok(records);
}

/// Validate a mutation journal against canonical event ranges and hashes.
///
/// The journal is disposable acceleration: callers may treat absence as an
/// empty journal, while corruption is reported distinctly from canonical-log
/// validity.
///
/// # Errors
///
/// Returns an error when a record is duplicated or disagrees with canonical
/// operation identifiers, event ranges, hashes, or successful response bounds.
pub fn validate_mutation_journal(session: &Path, log: &EventLog) -> Result<u64> {
    let path = session.join(MUTATION_JOURNAL_NAME);
    let records = read_mutation_journal(&path)?;
    let header_bytes = serde_json::to_vec(&log.header)
        .map_err(|source| return json_error(path.clone(), source))?;
    let header_hash = blake3::hash(&header_bytes).to_hex().to_string();
    let mut request_ids = HashSet::new();
    for record in &records {
        if !request_ids.insert(record.request_id) {
            return Err(invalid_journal(
                "mutation journal repeats a request identifier",
            ));
        }
        let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
        if record.event_end_seq > event_count {
            return Err(invalid_journal(
                "mutation journal event range exceeds the canonical log",
            ));
        }
        let expected_hash = if record.event_end_seq == 0 {
            header_hash.as_str()
        } else {
            let index = usize::try_from(record.event_end_seq - 1).map_err(|_| {
                return invalid_journal("mutation journal event boundary is not addressable");
            })?;
            log.events
                .get(index)
                .map(|event| return event.hash.as_str())
                .ok_or_else(|| {
                    return invalid_journal("mutation journal event boundary is missing");
                })?
        };
        if record.event_hash != expected_hash {
            return Err(invalid_journal(
                "mutation journal event hash does not match the canonical boundary",
            ));
        }
        if record.event_end_seq > record.event_start_seq {
            if record.operation_ids != [record.request_id]
                || !log.events.iter().any(|event| {
                    return event.seq >= record.event_start_seq
                        && event.seq < record.event_end_seq
                        && event_operation_id(&event.event) == Some(record.request_id);
                })
            {
                return Err(invalid_journal(
                    "mutation journal operation identifiers do not match its canonical range",
                ));
            }
        } else if !record.operation_ids.is_empty() {
            return Err(invalid_journal(
                "event-free mutation journal outcome names canonical operations",
            ));
        }
        if let ControlResult::Ok { data } = &record.response.result {
            let boundaries = match data {
                ControlData::Acknowledged { boundaries }
                | ControlData::Status { boundaries, .. }
                | ControlData::Marked { boundaries, .. } => boundaries,
            };
            if boundaries.observed.next_event_seq != record.event_end_seq
                || boundaries.observed.event_hash != record.event_hash
            {
                return Err(invalid_journal(
                    "mutation response boundary does not match its journaled canonical range",
                ));
            }
        }
    }
    return Ok(u64::try_from(records.len()).unwrap_or(u64::MAX));
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

fn hash_body(body: &MutationJournalBody<'_>, path: &Path) -> Result<String> {
    let bytes =
        serde_json::to_vec(body).map_err(|source| return json_error(path.to_path_buf(), source))?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn invalid_journal(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
