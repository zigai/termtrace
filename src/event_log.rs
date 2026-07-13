//! Crash-resistant append-only persistence and strict event-log recovery.

use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use blake3::Hash;
use uuid::Uuid;

use crate::{
    Error, Result,
    event::{
        EVENT_LOG_FORMAT, EVENT_LOG_VERSION, Event, EventBody, EventLogHeader, EventRecord,
        OperationStatus,
    },
};

const OWNER_DIRECTORY_MODE: u32 = 0o700;
const OWNER_FILE_MODE: u32 = 0o600;
const MAX_LINE_BYTES: u64 = 64 * 1024 * 1024;

/// Monotonic and wall-clock origins shared by one recording.
#[derive(Clone, Debug)]
pub struct SessionClock {
    monotonic_origin: Instant,
    started_unix_ns: u128,
}

impl SessionClock {
    /// Capture synchronized monotonic and wall-clock origins.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ClockBeforeEpoch`] when the system clock cannot be
    /// represented as a Unix timestamp.
    pub fn now() -> Result<Self> {
        let started_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::ClockBeforeEpoch)?
            .as_nanos();

        Ok(Self {
            monotonic_origin: Instant::now(),
            started_unix_ns,
        })
    }

    /// Wall-clock Unix timestamp corresponding to monotonic time zero.
    pub fn started_unix_ns(&self) -> u128 {
        self.started_unix_ns
    }

    /// Nanoseconds elapsed from the monotonic origin, saturating at `u64::MAX`.
    pub fn elapsed_ns(&self) -> u64 {
        u64::try_from(self.monotonic_origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// Append-only event-log writer that durably commits every acknowledged event.
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    clock: SessionClock,
    next_seq: u64,
    previous_hash: Hash,
    poisoned: bool,
}

impl EventLogWriter {
    /// Sequence number that will be assigned to the next event.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Create a new owner-only event log and durably write its header.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be secured, the file
    /// already exists, or the header cannot be serialized and synchronized.
    pub fn create(
        path: impl AsRef<Path>,
        header: &EventLogHeader,
        clock: SessionClock,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        validate_parent_directory(&path)?;
        validate_header(&path, header)?;
        if header.started_unix_ns != clock.started_unix_ns() {
            return Err(invalid_log(
                &path,
                1,
                "header wall-clock origin does not match the session clock",
            ));
        }

        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(OWNER_FILE_MODE)
            .open(&path)
            .map_err(|source| Error::Filesystem {
                path: path.clone(),
                source,
            })?;

        let header_bytes = serde_json::to_vec(header).map_err(|source| Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        })?;
        let previous_hash = blake3::hash(&header_bytes);
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&header_bytes)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush())
            .and_then(|()| writer.get_ref().sync_data())
            .map_err(|source| Error::Filesystem {
                path: path.clone(),
                source,
            })?;

        Ok(Self {
            path,
            writer,
            clock,
            next_seq: 0,
            previous_hash,
            poisoned: false,
        })
    }

    /// Append and durably synchronize an event before returning its sequence number.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, writing, flushing, synchronization,
    /// or sequence advancement fails.
    pub fn append(&mut self, event: Event) -> Result<u64> {
        self.append_at(self.clock.elapsed_ns(), event)
    }

    /// Append an event without an immediate `fsync`.
    ///
    /// Buffered events become durable with the next [`Self::append`] call.
    /// This is intended for high-volume PTY output; control intents and results
    /// must use [`Self::append`].
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or buffered writing fails, or when
    /// an earlier I/O failure poisoned the writer.
    pub fn append_buffered(&mut self, event: Event) -> Result<u64> {
        self.append_at_policy(self.clock.elapsed_ns(), event, false)
    }

    fn append_at(&mut self, monotonic_ns: u64, event: Event) -> Result<u64> {
        self.append_at_policy(monotonic_ns, event, true)
    }

    fn append_at_policy(&mut self, monotonic_ns: u64, event: Event, durable: bool) -> Result<u64> {
        if self.poisoned {
            return Err(Error::EventLogPoisoned {
                path: self.path.clone(),
            });
        }
        let seq = self.next_seq;
        let next_seq = seq.checked_add(1).ok_or_else(|| Error::InvalidEventLog {
            path: self.path.clone(),
            line: seq.saturating_add(2),
            reason: "event sequence overflow".to_owned(),
        })?;
        let body = EventBody {
            seq,
            monotonic_ns,
            event: &event,
        };
        let body_bytes = serde_json::to_vec(&body).map_err(|source| Error::InvalidJson {
            path: self.path.clone(),
            json_path: "$".to_owned(),
            source,
        })?;
        let hash = chained_hash(self.previous_hash, &body_bytes);
        let record = EventRecord {
            seq,
            monotonic_ns,
            prev_hash: self.previous_hash.to_hex().to_string(),
            hash: hash.to_hex().to_string(),
            event,
        };
        let record_bytes = serde_json::to_vec(&record).map_err(|source| Error::InvalidJson {
            path: self.path.clone(),
            json_path: "$".to_owned(),
            source,
        })?;

        let write_result = self
            .writer
            .write_all(&record_bytes)
            .and_then(|()| self.writer.write_all(b"\n"));
        let write_result = if durable {
            write_result
                .and_then(|()| self.writer.flush())
                .and_then(|()| self.writer.get_ref().sync_data())
        } else {
            write_result
        };
        if let Err(source) = write_result {
            self.poisoned = true;
            return Err(Error::Filesystem {
                path: self.path.clone(),
                source,
            });
        }

        self.next_seq = next_seq;
        self.previous_hash = hash;
        Ok(seq)
    }
}

/// Fully validated event-log contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventLog {
    /// Recording metadata.
    pub header: EventLogHeader,
    /// Ordered, integrity-checked events.
    pub events: Vec<EventRecord>,
    /// Intents without a corresponding result, usually due to interruption.
    pub pending_operations: Vec<PendingOperation>,
}

/// Kind of state-changing operation represented by an intent/result pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    /// Bytes sent to the child PTY.
    Input,
    /// Terminal geometry change.
    Resize,
    /// Process-group signal.
    Signal,
}

/// Durable operation intent whose delivery outcome is uncertain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingOperation {
    /// Operation identifier.
    pub operation_id: Uuid,
    /// Operation category.
    pub kind: OperationKind,
    /// Sequence containing the intent.
    pub intent_seq: u64,
    /// Intended byte count for input operations.
    pub expected_bytes: Option<u64>,
}

impl EventLog {
    /// Read and validate a complete event log.
    ///
    /// # Errors
    ///
    /// Returns an error for inaccessible files, malformed JSON, unsupported
    /// headers, truncation, non-contiguous ordering, time reversal, or a broken
    /// integrity chain.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })?;
        let mut reader = BufReader::new(file);

        let header_bytes = read_line(&mut reader, path, 1)?
            .ok_or_else(|| invalid_log(path, 1, "missing header"))?;
        let header: EventLogHeader = parse_json_line(path, 1, &header_bytes)?;
        validate_header(path, &header)?;

        let mut previous_hash = blake3::hash(&header_bytes);
        let mut previous_time = 0_u64;
        let mut expected_seq = 0_u64;
        let mut line_number = 2_u64;
        let mut events = Vec::new();
        let mut seen_operations = HashSet::new();
        let mut pending_operations = HashMap::new();

        while let Some(line) = read_line(&mut reader, path, line_number)? {
            let record: EventRecord = parse_json_line(path, line_number, &line)?;
            if record.seq != expected_seq {
                return Err(invalid_log(
                    path,
                    line_number,
                    format!("expected sequence {expected_seq}, found {}", record.seq),
                ));
            }
            if record.monotonic_ns < previous_time {
                return Err(invalid_log(
                    path,
                    line_number,
                    "monotonic timestamp moved backwards",
                ));
            }
            if record.prev_hash != previous_hash.to_hex().as_str() {
                return Err(invalid_log(
                    path,
                    line_number,
                    "previous hash does not match",
                ));
            }

            let body = EventBody {
                seq: record.seq,
                monotonic_ns: record.monotonic_ns,
                event: &record.event,
            };
            let body_bytes = serde_json::to_vec(&body).map_err(|source| Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            })?;
            let expected_hash = chained_hash(previous_hash, &body_bytes);
            if record.hash != expected_hash.to_hex().as_str() {
                return Err(invalid_log(path, line_number, "event hash does not match"));
            }
            validate_operation_transition(
                path,
                line_number,
                &record,
                &mut seen_operations,
                &mut pending_operations,
            )?;

            previous_hash = expected_hash;
            previous_time = record.monotonic_ns;
            expected_seq = expected_seq
                .checked_add(1)
                .ok_or_else(|| invalid_log(path, line_number, "event sequence overflow"))?;
            events.push(record);
            line_number = line_number.saturating_add(1);
        }

        let mut pending_operations = pending_operations.into_values().collect::<Vec<_>>();
        pending_operations.sort_by_key(|operation| operation.intent_seq);
        Ok(Self {
            header,
            events,
            pending_operations,
        })
    }
}

fn validate_operation_transition(
    path: &Path,
    line: u64,
    record: &EventRecord,
    seen: &mut HashSet<Uuid>,
    pending: &mut HashMap<Uuid, PendingOperation>,
) -> Result<()> {
    if let Some((operation_id, kind, expected_bytes)) = operation_intent(&record.event) {
        if !seen.insert(operation_id) {
            return Err(invalid_log(path, line, "operation identifier was reused"));
        }
        pending.insert(
            operation_id,
            PendingOperation {
                operation_id,
                kind,
                intent_seq: record.seq,
                expected_bytes,
            },
        );
        return Ok(());
    }

    let Some(result) = operation_result(&record.event) else {
        return Ok(());
    };
    let intent = pending
        .remove(&result.operation_id)
        .ok_or_else(|| invalid_log(path, line, "operation result has no pending intent"))?;
    if intent.kind != result.kind {
        return Err(invalid_log(
            path,
            line,
            "operation result kind does not match its intent",
        ));
    }
    match result.status {
        OperationStatus::Applied if result.error_code.is_some() => {
            return Err(invalid_log(
                path,
                line,
                "applied operation result contains an error code",
            ));
        }
        OperationStatus::Failed if result.error_code.is_none() => {
            return Err(invalid_log(
                path,
                line,
                "failed operation result is missing an error code",
            ));
        }
        OperationStatus::Applied | OperationStatus::Failed => {}
    }
    if let (Some(expected), Some(written)) = (intent.expected_bytes, result.bytes_written)
        && (written > expected
            || (result.status == OperationStatus::Applied && written != expected))
    {
        return Err(invalid_log(
            path,
            line,
            "input result byte count is inconsistent with its intent",
        ));
    }
    Ok(())
}

fn operation_intent(event: &Event) -> Option<(Uuid, OperationKind, Option<u64>)> {
    match event {
        Event::InputIntent {
            operation_id, data, ..
        } => Some((
            *operation_id,
            OperationKind::Input,
            Some(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX)),
        )),
        Event::ResizeIntent { operation_id, .. } => {
            Some((*operation_id, OperationKind::Resize, None))
        }
        Event::SignalIntent { operation_id, .. } => {
            Some((*operation_id, OperationKind::Signal, None))
        }
        _ => None,
    }
}

struct OperationResultRef<'a> {
    operation_id: Uuid,
    kind: OperationKind,
    status: OperationStatus,
    bytes_written: Option<u64>,
    error_code: &'a Option<String>,
}

fn operation_result(event: &Event) -> Option<OperationResultRef<'_>> {
    match event {
        Event::InputResult {
            operation_id,
            status,
            bytes_written,
            error_code,
        } => Some(OperationResultRef {
            operation_id: *operation_id,
            kind: OperationKind::Input,
            status: *status,
            bytes_written: Some(*bytes_written),
            error_code,
        }),
        Event::ResizeResult {
            operation_id,
            status,
            error_code,
        } => Some(OperationResultRef {
            operation_id: *operation_id,
            kind: OperationKind::Resize,
            status: *status,
            bytes_written: None,
            error_code,
        }),
        Event::SignalResult {
            operation_id,
            status,
            error_code,
        } => Some(OperationResultRef {
            operation_id: *operation_id,
            kind: OperationKind::Signal,
            status: *status,
            bytes_written: None,
            error_code,
        }),
        _ => None,
    }
}

fn validate_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error::Filesystem {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "event log has no parent"),
    })?;
    let metadata = fs_err::metadata(parent).map_err(|source| Error::Filesystem {
        path: parent.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(Error::Filesystem {
            path: parent.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "event log parent is not a directory",
            ),
        });
    }
    let mode = metadata.permissions().mode() & 0o777;
    let owner_uid = metadata.uid();
    if owner_uid != nix::unistd::Uid::effective().as_raw() || mode != OWNER_DIRECTORY_MODE {
        return Err(Error::InsecureArtifactDirectory {
            path: parent.to_path_buf(),
            owner_uid,
            mode,
        });
    }
    Ok(())
}

fn read_line(reader: &mut impl BufRead, path: &Path, line: u64) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
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
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let new_length = bytes.len().saturating_add(consumed);
        if u64::try_from(new_length).unwrap_or(u64::MAX) > MAX_LINE_BYTES {
            return Err(invalid_log(path, line, "line exceeds 64 MiB limit"));
        }
        bytes.extend_from_slice(&available[..consumed]);
        let complete = available[consumed - 1] == b'\n';
        reader.consume(consumed);
        if complete {
            bytes.pop();
            return Ok(Some(bytes));
        }
    }
}

fn parse_json_line<T>(path: &Path, line: u64, bytes: &[u8]) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| {
            let json_path = error.path().to_string();
            let source = error.into_inner();
            Error::InvalidJson {
                path: path.to_path_buf(),
                json_path,
                source,
            }
        })
        .map_err(|error| match error {
            Error::InvalidJson {
                json_path, source, ..
            } => Error::InvalidEventLog {
                path: path.to_path_buf(),
                line,
                reason: format!("JSON decoding failed at `{json_path}`: {source}"),
            },
            other => other,
        })
}

fn validate_header(path: &Path, header: &EventLogHeader) -> Result<()> {
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
    if header.cols == 0 || header.rows == 0 {
        return Err(invalid_log(path, 1, "terminal dimensions must be nonzero"));
    }
    if header.term.is_empty() {
        return Err(invalid_log(path, 1, "terminal identity must not be empty"));
    }
    Ok(())
}

fn chained_hash(previous: Hash, body: &[u8]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(previous.as_bytes());
    hasher.update(body);
    hasher.finalize()
}

fn invalid_log(path: &Path, line: u64, reason: impl Into<String>) -> Error {
    Error::InvalidEventLog {
        path: path.to_path_buf(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;
    use crate::event::{ByteString, InputKind, InputSource, OperationStatus};

    fn header(clock: &SessionClock) -> EventLogHeader {
        EventLogHeader::new(
            Uuid::now_v7(),
            clock.started_unix_ns(),
            120,
            40,
            1080,
            720,
            "xterm-ghostty",
        )
    }

    fn secure_directory(path: &Path) {
        fs_err::create_dir(path).expect("create secure directory");
        fs_err::set_permissions(path, Permissions::from_mode(OWNER_DIRECTORY_MODE))
            .expect("secure directory permissions");
    }

    #[test]
    fn round_trip_preserves_binary_events_and_integrity_chain() {
        let temporary = TempDir::new().expect("temporary directory");
        let path = temporary.path().join("session/events.jsonl");
        secure_directory(path.parent().expect("parent"));
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");

        writer
            .append_at(
                10,
                Event::Output {
                    data: ByteString::from(&b"\x1b[31m\xff"[..]),
                },
            )
            .expect("append output");
        let input_id = Uuid::now_v7();
        writer
            .append_at(
                20,
                Event::InputIntent {
                    operation_id: input_id,
                    source: InputSource::Agent,
                    input_kind: InputKind::Keys,
                    data: ByteString::from(&b"\x03"[..]),
                    intent: Some("ctrl-c".to_owned()),
                },
            )
            .expect("append input");
        writer
            .append_at(
                21,
                Event::InputResult {
                    operation_id: input_id,
                    status: OperationStatus::Applied,
                    bytes_written: 1,
                    error_code: None,
                },
            )
            .expect("append input result");
        drop(writer);

        let recording = EventLog::read(&path).expect("read valid recording");
        assert_eq!(recording.events.len(), 3);
        assert!(recording.pending_operations.is_empty());
        assert_eq!(recording.events[0].seq, 0);
        assert_eq!(recording.events[1].seq, 1);
        assert!(matches!(
            &recording.events[0].event,
            Event::Output { data } if data.as_bytes() == b"\x1b[31m\xff"
        ));

        let directory_mode = fs_err::metadata(path.parent().expect("parent"))
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs_err::metadata(&path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, OWNER_DIRECTORY_MODE);
        assert_eq!(file_mode, OWNER_FILE_MODE);
    }

    #[test]
    fn reports_intent_without_result_as_uncertain() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
        let operation_id = Uuid::now_v7();
        writer
            .append_at(
                1,
                Event::InputIntent {
                    operation_id,
                    source: InputSource::Agent,
                    input_kind: InputKind::Text,
                    data: ByteString::from(&b"hello"[..]),
                    intent: None,
                },
            )
            .expect("append intent");
        drop(writer);

        let recording = EventLog::read(&path).expect("read interrupted recording");
        assert_eq!(
            recording.pending_operations,
            vec![PendingOperation {
                operation_id,
                kind: OperationKind::Input,
                intent_seq: 0,
                expected_bytes: Some(5),
            }]
        );
    }

    #[test]
    fn rejects_result_without_matching_intent() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
        writer
            .append_at(
                1,
                Event::InputResult {
                    operation_id: Uuid::now_v7(),
                    status: OperationStatus::Applied,
                    bytes_written: 0,
                    error_code: None,
                },
            )
            .expect("append unmatched result");
        drop(writer);

        let error = EventLog::read(&path).expect_err("unmatched result must fail");
        assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
    }

    #[test]
    fn rejects_tampered_event_payload() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
        writer
            .append_at(
                1,
                Event::Marker {
                    label: "before".to_owned(),
                },
            )
            .expect("append marker");
        drop(writer);

        let original = fs_err::read_to_string(&path).expect("read log");
        fs_err::write(&path, original.replace("before", "after!")).expect("tamper log");

        let error = EventLog::read(&path).expect_err("tampering must fail");
        assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
    }

    #[test]
    fn rejects_truncated_last_event() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
        writer
            .append_at(
                1,
                Event::Marker {
                    label: "complete".to_owned(),
                },
            )
            .expect("append marker");
        drop(writer);

        let mut bytes = fs_err::read(&path).expect("read log");
        bytes.pop();
        fs_err::write(&path, bytes).expect("truncate log");

        let error = EventLog::read(&path).expect_err("truncation must fail");
        assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
    }

    #[test]
    fn refuses_group_readable_artifact_directory() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        fs_err::create_dir(&directory).expect("create directory");
        fs_err::set_permissions(&directory, Permissions::from_mode(0o750))
            .expect("set insecure permissions");
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");

        let error = EventLogWriter::create(&path, &header(&clock), clock)
            .expect_err("insecure directory must fail");
        assert!(matches!(error, Error::InsecureArtifactDirectory { .. }));
        assert!(!path.exists());
    }

    #[test]
    fn rejects_malformed_base64_with_event_path() {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
        writer
            .append_at(
                1,
                Event::Output {
                    data: ByteString::from(&b"valid"[..]),
                },
            )
            .expect("append output");
        drop(writer);

        let original = fs_err::read_to_string(&path).expect("read log");
        fs_err::write(&path, original.replace("dmFsaWQ=", "%%%=")).expect("write malformed log");

        let error = EventLog::read(&path).expect_err("malformed Base64 must fail");
        match error {
            Error::InvalidEventLog { reason, .. } => {
                assert!(reason.contains("`event`"), "unexpected reason: {reason}");
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
