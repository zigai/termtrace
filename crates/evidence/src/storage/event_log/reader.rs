use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata, OpenOptions, symlink_metadata},
    io::{BufRead, BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use blake3::Hash;
use nix::libc::O_NOFOLLOW;
use termtrace_model::{
    execution::event::{Event, EventBody, EventLogHeader, EventRecord},
    execution::event_log::{
        OperationKind, PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport,
    },
    protocol::control::LiveEventCursor,
};
use uuid::Uuid;

use crate::storage::{Error, Result};

use super::{
    operations::validate_operation_transition,
    types::{EventLog, EventLogSummary},
    validation::{
        MAX_LINE_BYTES, OWNER_FILE_MODE, chained_hash, invalid_log, parse_json_line, read_line,
        validate_event_geometry, validate_header, validate_parent_directory,
    },
};

const MAX_EVENT_COUNT: u64 = 1_000_000;
const MAX_EVENT_LOG_ENCODED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EVENT_LOG_DECODED_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct EventLogLimits {
    pub(super) event_count: u64,
    pub(super) encoded_bytes: u64,
    pub(super) decoded_payload_bytes: u64,
}
pub(super) const DEFAULT_EVENT_LOG_LIMITS: EventLogLimits = EventLogLimits {
    event_count: MAX_EVENT_COUNT,
    encoded_bytes: MAX_EVENT_LOG_ENCODED_BYTES,
    decoded_payload_bytes: MAX_EVENT_LOG_DECODED_PAYLOAD_BYTES,
};

// Streaming work limits are distinct from the materializing reader's allocation limits.
const STREAM_EVENT_LOG_LIMITS: EventLogLimits = EventLogLimits {
    event_count: 2 * MAX_EVENT_COUNT,
    encoded_bytes: 4 * 1024 * 1024 * 1024,
    decoded_payload_bytes: 2 * 1024 * 1024 * 1024,
};

#[derive(Debug)]
pub(super) struct ParsedEventLog {
    log: EventLog,
    valid_lines: u64,
    issue: Option<RecoveryIssue>,
}

/// Streaming validator over canonical event records.
pub struct EventLogCursor {
    path: PathBuf,
    header: EventLogHeader,
    header_hash: Hash,
    file: File,
    opened_metadata: Metadata,
    validation: LogValidation,
    line_number: u64,
    encoded_bytes: u64,
    decoded_payload_bytes: u64,
    buffer: Vec<u8>,
    buffer_start: usize,
}

impl EventLog {
    /// Read and validate a complete event log.
    ///
    /// # Errors
    ///
    /// Returns an error when the file violates size, format, sequencing, or hash-chain limits.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        return Ok(parse_event_log(path.as_ref(), false, None)?.log);
    }
    /// Read and validate exactly the requested event prefix.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested prefix is unavailable or invalid.
    pub fn read_prefix(path: impl AsRef<Path>, event_count: u64) -> Result<Self> {
        return Ok(parse_event_log(path.as_ref(), false, Some(event_count))?.log);
    }
    /// Copy the longest valid event prefix into a new owner-only event log.
    ///
    /// # Errors
    ///
    /// Returns an error when the source cannot be recovered or the destination cannot be created
    /// and verified.
    pub fn recover_prefix(
        source: impl AsRef<Path>,
        recovered: impl AsRef<Path>,
    ) -> Result<RecoveryReport> {
        let source = source.as_ref();
        let recovered = recovered.as_ref();
        let parsed = parse_event_log(source, true, None)?;
        validate_parent_directory(recovered)?;
        copy_lines(source, recovered, parsed.valid_lines)?;
        let verified = Self::read(recovered)?;
        let pending_operations: Vec<PendingOperationReport> = verified
            .pending_operations
            .iter()
            .map(|operation| {
                return PendingOperationReport {
                    operation_id: operation.operation_id,
                    kind: match operation.kind {
                        OperationKind::Input => "input",
                        OperationKind::Resize => "resize",
                        OperationKind::Signal => "signal",
                    }
                    .to_owned(),
                    intent_seq: operation.intent_seq,
                    expected_bytes: operation.expected_bytes,
                };
            })
            .collect();
        return Ok(RecoveryReport {
            source: source.to_path_buf(),
            recovered: recovered.to_path_buf(),
            valid_events: u64::try_from(verified.events.len()).unwrap_or(u64::MAX),
            discarded_tail: parsed.issue,
            pending_operations,
        });
    }
}

struct LogValidation {
    previous_hash: Hash,
    previous_time: u64,
    expected_seq: u64,
    seen_operations: HashSet<Uuid>,
    pending_operations: HashMap<Uuid, PendingOperation>,
    validate_operations: bool,
}

impl LogValidation {
    fn validate_record(&mut self, path: &Path, line: u64, bytes: &[u8]) -> Result<EventRecord> {
        let record: EventRecord = parse_json_line(path, line, bytes)?;
        validate_event_geometry(path, line, &record.event)?;
        if record.seq != self.expected_seq {
            return Err(invalid_log(
                path,
                line,
                format!(
                    "expected sequence {}, found {}",
                    self.expected_seq, record.seq
                ),
            ));
        }
        if record.monotonic_ns < self.previous_time {
            return Err(invalid_log(
                path,
                line,
                "monotonic timestamp moved backwards",
            ));
        }
        if record.prev_hash != self.previous_hash.to_hex().as_str() {
            return Err(invalid_log(path, line, "previous hash does not match"));
        }
        let body = EventBody {
            seq: record.seq,
            monotonic_ns: record.monotonic_ns,
            event: &record.event,
        };
        let body_bytes = serde_json::to_vec(&body).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let expected_hash = chained_hash(self.previous_hash, &body_bytes);
        if record.hash != expected_hash.to_hex().as_str() {
            return Err(invalid_log(path, line, "event hash does not match"));
        }
        if self.validate_operations {
            validate_operation_transition(
                path,
                line,
                &record,
                &mut self.seen_operations,
                &mut self.pending_operations,
            )?;
        }
        self.previous_hash = expected_hash;
        self.previous_time = record.monotonic_ns;
        self.expected_seq = self
            .expected_seq
            .checked_add(1)
            .ok_or_else(|| return invalid_log(path, line, "event sequence overflow"))?;
        return Ok(record);
    }
}

impl EventLogCursor {
    /// Open a cursor after validating the canonical event-log header.
    ///
    /// # Errors
    ///
    /// Returns an error when the file or header is invalid.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
        let opened_metadata = file.metadata().map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if !opened_metadata.is_file() {
            return Err(invalid_log(&path, 1, "event log must be a regular file"));
        }
        let mut reader = BufReader::new(file);
        let header_bytes = read_line(&mut reader, &path, 1)?
            .ok_or_else(|| return invalid_log(&path, 1, "missing header"))?;
        let encoded_bytes = u64::try_from(header_bytes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if encoded_bytes > STREAM_EVENT_LOG_LIMITS.encoded_bytes {
            return Err(invalid_log(
                &path,
                1,
                format!(
                    "event log exceeds {} encoded-byte limit",
                    STREAM_EVENT_LOG_LIMITS.encoded_bytes
                ),
            ));
        }
        let header: EventLogHeader = parse_json_line(&path, 1, &header_bytes)?;
        validate_header(&path, &header)?;
        let position = reader.stream_position().map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        let mut file = reader.into_inner();
        file.seek(SeekFrom::Start(position)).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        let header_hash = blake3::hash(&header_bytes);
        return Ok(Self {
            path,
            header,
            header_hash,
            file,
            opened_metadata,
            validation: LogValidation {
                previous_hash: header_hash,
                previous_time: 0,
                expected_seq: 0,
                seen_operations: HashSet::new(),
                pending_operations: HashMap::new(),
                validate_operations: true,
            },
            line_number: 2,
            encoded_bytes,
            decoded_payload_bytes: 0,
            buffer: Vec::new(),
            buffer_start: 0,
        });
    }
    /// Seek to a position supplied by the live actor after checking its source identity.
    ///
    /// This validates only a suffix: it can never finish as full-log operation verification.
    /// The caller must obtain the position from the live actor, not imported file metadata,
    /// and validate the suffix against a later actor boundary before publishing records.
    pub fn resume_live_suffix(
        &mut self,
        position: &LiveEventCursor,
        device: u64,
        inode: u64,
        header_hash: &str,
    ) -> Result<()> {
        self.validate_live_source(device, inode, header_hash)?;
        if self.validation.expected_seq != 0 || position.file_offset < self.encoded_bytes {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "invalid live resume position",
            ));
        }
        let hash = Hash::from_hex(&position.event_hash).map_err(|_| {
            return invalid_log(&self.path, self.line_number, "invalid live cursor hash");
        })?;
        self.file
            .seek(SeekFrom::Start(position.file_offset))
            .map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;
        self.validation.previous_hash = hash;
        self.validation.previous_time = position.monotonic_ns;
        self.validation.expected_seq = position.next_event_seq;
        self.validation.validate_operations = false;
        self.line_number = position.next_event_seq.saturating_add(2);
        self.encoded_bytes = position.file_offset;
        self.buffer.clear();
        self.buffer_start = 0;
        return Ok(());
    }

    /// Require the path still names the actor's descriptor and the same canonical header.
    pub fn validate_live_source(&self, device: u64, inode: u64, header_hash: &str) -> Result<()> {
        if self.opened_metadata.dev() != device
            || self.opened_metadata.ino() != inode
            || self.header_hash.to_hex().as_str() != header_hash
        {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "live event source identity does not match actor",
            ));
        }
        return self.validate_source_identity();
    }

    /// Reject replacement, symlinks, and truncation of the open source.
    pub fn validate_source_identity(&self) -> Result<()> {
        let current = symlink_metadata(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        let opened = self.file.metadata().map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        if !current.is_file()
            || current.dev() != self.opened_metadata.dev()
            || current.ino() != self.opened_metadata.ino()
            || opened.len() < self.position_offset()
            || current.len() < self.position_offset()
        {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "event source was replaced or truncated",
            ));
        }
        return Ok(());
    }

    fn position_offset(&self) -> u64 {
        return self.encoded_bytes.saturating_sub(
            u64::try_from(self.buffer.len().saturating_sub(self.buffer_start)).unwrap_or(u64::MAX),
        );
    }

    /// Next sequence validated by this cursor.
    pub fn next_event_seq(&self) -> u64 {
        return self.validation.expected_seq;
    }

    /// Require the validated suffix to end at an independently trusted actor position.
    pub fn validate_live_boundary(&self, boundary: &LiveEventCursor) -> Result<()> {
        self.validate_boundary(boundary.next_event_seq, &boundary.event_hash)?;
        if self.position_offset() != boundary.file_offset
            || self.validation.previous_time != boundary.monotonic_ns
        {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "live event cursor offset or timestamp differs from actor",
            ));
        }
        return Ok(());
    }

    /// Compare a validated chain endpoint with a separately validated authority.
    pub fn validate_boundary(&self, next_event_seq: u64, event_hash: &str) -> Result<()> {
        self.validate_source_identity()?;
        if self.validation.expected_seq != next_event_seq
            || self.validation.previous_hash.to_hex().as_str() != event_hash
        {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "event suffix does not match trusted boundary",
            ));
        }
        return Ok(());
    }

    /// Read and validate the next complete event record.
    ///
    /// # Errors
    ///
    /// Returns an error when the next record violates resource, format, or hash-chain limits.
    pub fn next_record(&mut self) -> Result<Option<EventRecord>> {
        loop {
            let buffered = self.buffer.get(self.buffer_start..).ok_or_else(|| {
                return invalid_log(&self.path, self.line_number, "invalid cursor");
            })?;
            if let Some(relative_end) = buffered.iter().position(|byte| return *byte == b'\n') {
                let line_end = self.buffer_start.saturating_add(relative_end);
                if u64::try_from(relative_end).unwrap_or(u64::MAX) > MAX_LINE_BYTES {
                    return Err(invalid_log(
                        &self.path,
                        self.line_number,
                        "line exceeds 64 MiB limit",
                    ));
                }
                if self.validation.expected_seq >= STREAM_EVENT_LOG_LIMITS.event_count {
                    return Err(invalid_log(
                        &self.path,
                        self.line_number,
                        format!(
                            "event log exceeds {} event limit",
                            STREAM_EVENT_LOG_LIMITS.event_count
                        ),
                    ));
                }
                let record = self.validation.validate_record(
                    &self.path,
                    self.line_number,
                    self.buffer
                        .get(self.buffer_start..line_end)
                        .ok_or_else(|| {
                            return invalid_log(
                                &self.path,
                                self.line_number,
                                "invalid cursor range",
                            );
                        })?,
                )?;
                let decoded_payload_bytes = self
                    .decoded_payload_bytes
                    .saturating_add(record.event.decoded_payload_bytes());
                if decoded_payload_bytes > STREAM_EVENT_LOG_LIMITS.decoded_payload_bytes {
                    return Err(invalid_log(
                        &self.path,
                        self.line_number,
                        format!(
                            "event log exceeds {} decoded-payload-byte limit",
                            STREAM_EVENT_LOG_LIMITS.decoded_payload_bytes
                        ),
                    ));
                }
                self.decoded_payload_bytes = decoded_payload_bytes;
                self.buffer_start = line_end.saturating_add(1);
                self.line_number = self.line_number.saturating_add(1);
                if self.buffer_start == self.buffer.len() {
                    self.buffer.clear();
                    self.buffer_start = 0;
                }
                return Ok(Some(record));
            }
            if u64::try_from(buffered.len()).unwrap_or(u64::MAX) > MAX_LINE_BYTES {
                return Err(invalid_log(
                    &self.path,
                    self.line_number,
                    "line exceeds 64 MiB limit",
                ));
            }
            if self.buffer_start > 0 {
                self.buffer.drain(..self.buffer_start);
                self.buffer_start = 0;
            }
            let mut chunk = [0_u8; 8192];
            let count = match self.file.read(&mut chunk) {
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(Error::Filesystem {
                        path: self.path.clone(),
                        source,
                    });
                }
            };
            if count == 0 {
                return Ok(None);
            }
            self.encoded_bytes = self
                .encoded_bytes
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            if self.encoded_bytes > STREAM_EVENT_LOG_LIMITS.encoded_bytes {
                return Err(invalid_log(
                    &self.path,
                    self.line_number,
                    format!(
                        "event log exceeds {} encoded-byte limit",
                        STREAM_EVENT_LOG_LIMITS.encoded_bytes
                    ),
                ));
            }
            self.buffer
                .extend_from_slice(chunk.get(..count).ok_or_else(|| {
                    return invalid_log(&self.path, self.line_number, "invalid read length");
                })?);
        }
    }

    /// The validated canonical header.
    pub fn header(&self) -> &EventLogHeader {
        return &self.header;
    }

    /// Require a complete file rather than an appendable live prefix.
    pub fn finish(self) -> Result<Vec<PendingOperation>> {
        if !self.validation.validate_operations {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "a live suffix cursor cannot verify full-log operations",
            ));
        }
        if let Some(line) = self.incomplete_line() {
            return Err(invalid_log(
                &self.path,
                line,
                "unterminated trailing record",
            ));
        }
        let finished = self.file.metadata().map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        let current = symlink_metadata(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        let identity = |metadata: &Metadata| {
            return (
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            );
        };
        if identity(&self.opened_metadata) != identity(&finished)
            || identity(&finished) != identity(&current)
        {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "event log changed during content validation",
            ));
        }
        let mut pending: Vec<_> = self.validation.pending_operations.into_values().collect();
        pending.sort_by_key(|operation| return operation.intent_seq);
        return Ok(pending);
    }

    /// One-based line number of an unterminated trailing record, when buffered.
    pub fn incomplete_line(&self) -> Option<u64> {
        return (!self.buffer.is_empty()).then_some(self.line_number);
    }
}

impl EventLogSummary {
    /// Validate every canonical record while retaining only the final record and counters.
    ///
    /// # Errors
    ///
    /// Rejects malformed, incomplete, over-budget, or semantically invalid hash chains.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        return Self::read_with_check(path, || return Ok(()));
    }

    /// Stream validation with a cooperative cancellation/resource check before every record.
    ///
    /// # Errors
    ///
    /// Returns any check failure or canonical validation error.
    pub fn read_with_check(
        path: impl AsRef<Path>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut cursor = EventLogCursor::open(path)?;
        let mut summary = Self {
            header: cursor.header().clone(),
            event_count: 0,
            output_bytes: 0,
            terminal: None,
            pending_operations: Vec::new(),
            resource_triggers: [false; 3],
        };
        loop {
            check()?;
            let Some(record) = cursor.next_record()? else {
                break;
            };
            summary.event_count += 1;
            match &record.event {
                Event::Output { data } => {
                    summary.output_bytes +=
                        u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX);
                }
                Event::HostError { code, .. } => match code.as_str() {
                    "output_budget_exceeded" => summary.resource_triggers[0] = true,
                    "max_duration_exceeded" => summary.resource_triggers[1] = true,
                    "idle_timeout_exceeded" => summary.resource_triggers[2] = true,
                    _ => {}
                },
                _ => {}
            }
            summary.terminal = Some(record);
        }
        summary.pending_operations = cursor.finish()?;
        return Ok(summary);
    }
}

fn parse_event_log(
    path: &Path,
    recover_tail: bool,
    event_limit: Option<u64>,
) -> Result<ParsedEventLog> {
    return parse_event_log_with_limits(path, recover_tail, event_limit, DEFAULT_EVENT_LOG_LIMITS);
}

pub(super) fn parse_event_log_with_limits(
    path: &Path,
    recover_tail: bool,
    event_limit: Option<u64>,
    limits: EventLogLimits,
) -> Result<ParsedEventLog> {
    if event_limit.is_some_and(|limit| return limit > limits.event_count) {
        return Err(invalid_log(
            path,
            1,
            format!(
                "requested event prefix exceeds {} event limit",
                limits.event_count
            ),
        ));
    }
    let file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mut reader = BufReader::new(file);
    let header_bytes = read_line(&mut reader, path, 1)?
        .ok_or_else(|| return invalid_log(path, 1, "missing header"))?;
    let mut encoded_bytes = u64::try_from(header_bytes.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    if encoded_bytes > limits.encoded_bytes {
        return Err(invalid_log(
            path,
            1,
            format!(
                "event log exceeds {} encoded-byte limit",
                limits.encoded_bytes
            ),
        ));
    }
    let header: EventLogHeader = parse_json_line(path, 1, &header_bytes)?;
    validate_header(path, &header)?;
    let mut validation = LogValidation {
        previous_hash: blake3::hash(&header_bytes),
        previous_time: 0,
        expected_seq: 0,
        seen_operations: HashSet::new(),
        pending_operations: HashMap::new(),
        validate_operations: true,
    };
    let mut events: Vec<EventRecord> = Vec::new();
    let mut decoded_payload_bytes = 0_u64;
    let mut line_number = 2_u64;
    let issue = loop {
        if event_limit == Some(u64::try_from(events.len()).unwrap_or(u64::MAX)) {
            break None;
        }
        let line = match read_line(&mut reader, path, line_number) {
            Ok(Some(line)) => line,
            Ok(None) => {
                if let Some(limit) = event_limit {
                    return Err(invalid_log(
                        path,
                        line_number,
                        format!(
                            "requested {limit} prefix events, found only {}",
                            events.len()
                        ),
                    ));
                }
                break None;
            }
            Err(error) if recover_tail => break Some(recovery_issue(&error, line_number)),
            Err(error) => return Err(error),
        };
        encoded_bytes = encoded_bytes.saturating_add(
            u64::try_from(line.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        );
        if encoded_bytes > limits.encoded_bytes {
            let error = invalid_log(
                path,
                line_number,
                format!(
                    "event log exceeds {} encoded-byte limit",
                    limits.encoded_bytes
                ),
            );
            if recover_tail {
                break Some(recovery_issue(&error, line_number));
            }
            return Err(error);
        }
        if u64::try_from(events.len()).unwrap_or(u64::MAX) >= limits.event_count {
            let error = invalid_log(
                path,
                line_number,
                format!("event log exceeds {} event limit", limits.event_count),
            );
            if recover_tail {
                break Some(recovery_issue(&error, line_number));
            }
            return Err(error);
        }
        match validation.validate_record(path, line_number, &line) {
            Ok(record) => {
                decoded_payload_bytes =
                    decoded_payload_bytes.saturating_add(record.event.decoded_payload_bytes());
                if decoded_payload_bytes > limits.decoded_payload_bytes {
                    let error = invalid_log(
                        path,
                        line_number,
                        format!(
                            "event log exceeds {} decoded-payload-byte limit",
                            limits.decoded_payload_bytes
                        ),
                    );
                    if recover_tail {
                        break Some(recovery_issue(&error, line_number));
                    }
                    return Err(error);
                }
                events.push(record);
            }
            Err(error) if recover_tail => break Some(recovery_issue(&error, line_number)),
            Err(error) => return Err(error),
        }
        line_number = line_number.saturating_add(1);
    };
    let mut pending_operations: Vec<PendingOperation> =
        validation.pending_operations.into_values().collect();
    pending_operations.sort_by_key(|operation| return operation.intent_seq);
    return Ok(ParsedEventLog {
        valid_lines: u64::try_from(events.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
        log: EventLog {
            header,
            events,
            pending_operations,
            source_path: Some(path.to_path_buf()),
        },
        issue,
    });
}

fn recovery_issue(error: &Error, fallback_line: u64) -> RecoveryIssue {
    match error {
        Error::InvalidEventLog { line, reason, .. } => {
            return RecoveryIssue {
                line: *line,
                reason: reason.clone(),
            };
        }
        _ => {
            return RecoveryIssue {
                line: fallback_line,
                reason: error.to_string(),
            };
        }
    }
}

fn copy_lines(source: &Path, recovered: &Path, count: u64) -> Result<()> {
    let input = File::open(source).map_err(|source_error| {
        return Error::Filesystem {
            path: source.to_path_buf(),
            source: source_error,
        };
    })?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_FILE_MODE)
        .open(recovered)
        .map_err(|source| {
            return Error::Filesystem {
                path: recovered.to_path_buf(),
                source,
            };
        })?;
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);
    for line in 1..=count {
        let mut bytes: Vec<u8> = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|source_error| {
                return Error::Filesystem {
                    path: source.to_path_buf(),
                    source: source_error,
                };
            })?;
        if read == 0 || bytes.last() != Some(&b'\n') {
            return Err(invalid_log(source, line, "valid prefix copy was truncated"));
        }
        writer.write_all(&bytes).map_err(|source| {
            return Error::Filesystem {
                path: recovered.to_path_buf(),
                source,
            };
        })?;
    }
    return writer
        .flush()
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: recovered.to_path_buf(),
                source,
            };
        });
}
