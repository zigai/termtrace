use super::{
    model::{EventLog, EventLogSummary},
    operations::validate_operation_transition,
    segments::{canonical_source_fingerprint, CanonicalLogFile, SegmentWriter},
    validation::{
        invalid_log, parse_json_line, read_line, validate_event_geometry, validate_header,
        validate_parent_directory, MAX_LINE_BYTES, OWNER_FILE_MODE,
    },
};
use crate::storage::{
    artifact::sync_parent_directory, identifier_index::IdentifierSet, Error, Result,
};
use blake3::Hash;
use std::{
    collections::HashMap,
    fs::{symlink_metadata, Metadata, OpenOptions},
    io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};
use termtrace_model::{
    execution::event::{Event, EventBody, EventLogHeader, EventLogPosition, EventRecord},
    execution::event_log::{
        OperationKind, PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport,
    },
    protocol::control::LiveEventCursor,
};
use uuid::Uuid;

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
// Streaming validation retains one bounded record and a disk-backed identifier index.
// The materializing reader keeps its allocation limits; lifetime streaming has none.
const STREAM_EVENT_LOG_LIMITS: EventLogLimits = EventLogLimits {
    event_count: u64::MAX,
    encoded_bytes: u64::MAX,
    decoded_payload_bytes: u64::MAX,
};

/// Streaming validator over canonical event records.
pub struct EventLogCursor {
    path: PathBuf,
    header: EventLogHeader,
    header_hash: Hash,
    file: CanonicalLogFile,
    opened_metadata: Metadata,
    opened_fingerprint: String,
    validation: LogValidation,
    line_number: u64,
    encoded_bytes: u64,
    decoded_payload_bytes: u64,
    buffer: Vec<u8>,
    buffer_start: usize,
    snapshot_end: Option<u64>,
}

impl EventLog {
    /// Read a bounded event page from a verified finite snapshot of an appendable log.
    ///
    /// At most 4096 records and 8 MiB of encoded records are retained, except
    /// that a single valid record may exceed the page byte target. Later
    /// appends and a partial trailing record are excluded. The returned page
    /// does not claim durability or session completion.
    ///
    /// # Errors
    /// Rejects an invalid page limit, invalid complete records, or source replacement/truncation.
    pub fn read_page(
        path: impl AsRef<Path>,
        from_event_seq: u64,
        limit: u16,
    ) -> Result<Vec<EventRecord>> {
        if !(1..=4096).contains(&limit) {
            return Err(invalid_log(
                path.as_ref(),
                1,
                "event page limit must be from 1 through 4096",
            ));
        }

        let mut cursor = EventLogCursor::open_snapshot(path)?;
        let mut records = Vec::new();
        let mut bytes = 0_u64;
        let mut previous_position = cursor.position_offset();
        let mut full = false;

        while let Some(record) = cursor.next_record()? {
            let position = cursor.position_offset();
            let record_bytes = position.saturating_sub(previous_position);
            previous_position = position;

            if record.seq >= from_event_seq && records.len() < usize::from(limit) && !full {
                let next_bytes = bytes.saturating_add(record_bytes);

                if !records.is_empty() && next_bytes > 8 * 1024 * 1024 {
                    full = true;
                } else {
                    bytes = next_bytes;
                    records.push(record);
                }
            }
        }

        let _pending = cursor.finish_snapshot()?;

        return Ok(records);
    }

    /// Read and validate a complete event log.
    ///
    /// # Errors
    ///
    /// Returns an error when the file violates size, format, sequencing, or hash-chain limits.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        return parse_event_log(path.as_ref(), None);
    }

    /// Read and validate exactly the requested event prefix.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested prefix is unavailable or invalid.
    pub fn read_prefix(path: impl AsRef<Path>, event_count: u64) -> Result<Self> {
        return parse_event_log(path.as_ref(), Some(event_count));
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
        let discarded_tail = recover_stream(source, recovered)?;
        let verified = EventLogSummary::read(recovered)?;
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
            valid_events: verified.event_count,
            discarded_tail,
            pending_operations,
        });
    }
}

struct LogValidation {
    previous_hash: Hash,
    previous_time: u64,
    expected_seq: u64,
    seen_operations: IdentifierSet,
    pending_operations: HashMap<Uuid, PendingOperation>,
    validate_operations: bool,
}

impl LogValidation {
    fn validate_record(&mut self, path: &Path, line: u64, bytes: &[u8]) -> Result<EventRecord> {
        let record: EventRecord = parse_json_line(path, line, bytes)?;
        if record
            .observed_monotonic_ns
            .is_some_and(|observed| return observed > record.monotonic_ns)
        {
            return Err(invalid_log(path, line, "observation is after append time"));
        }

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
            observed_monotonic_ns: record.observed_monotonic_ns,
            event: &record.event,
        };

        let mut hasher = blake3::Hasher::new();
        hasher.update(self.previous_hash.as_bytes());
        serde_json::to_writer(&mut hasher, &body).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let expected_hash = hasher.finalize();

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
        let file = CanonicalLogFile::open(&path).map_err(|source| {
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
        let opened_fingerprint = canonical_source_fingerprint(&path).map_err(|source| {
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
            opened_fingerprint,
            validation: LogValidation {
                previous_hash: header_hash,
                previous_time: 0,
                expected_seq: 0,
                seen_operations: IdentifierSet::default(),
                pending_operations: HashMap::new(),
                validate_operations: true,
            },
            line_number: 2,
            encoded_bytes,
            decoded_payload_bytes: 0,
            buffer: Vec::new(),
            buffer_start: 0,
            snapshot_end: None,
        });
    }

    /// Open a finite snapshot of the complete records present at the current byte boundary.
    ///
    /// Later appends are excluded. An unfinished trailing record remains an explicit
    /// incomplete prefix and must be finalized with `finish_snapshot`, not `finish`.
    ///
    /// # Errors
    /// Returns an error for an invalid header or unreadable canonical source.
    pub fn open_snapshot(path: impl AsRef<Path>) -> Result<Self> {
        let mut cursor = Self::open(path)?;
        cursor.snapshot_end = Some(cursor.file.len().map_err(|source| {
            return Error::Filesystem {
                path: cursor.path.clone(),
                source,
            };
        })?);

        return Ok(cursor);
    }

    /// Seek to a position supplied by the live actor after checking its source identity.
    ///
    /// This validates only a suffix: it can never finish as full-log operation verification.
    /// The caller must obtain the position from the live actor, not imported file metadata,
    /// and validate the suffix against a later actor boundary before publishing records.
    ///
    /// # Errors
    ///
    /// Returns an error when the source identity differs from the actor's, the cursor has already
    /// read records or `position` precedes the bytes read so far, the cursor hash is not valid
    /// BLAKE3 hex, or seeking fails.
    pub fn resume_live_suffix(
        &mut self,
        position: &LiveEventCursor,
        device: u64,
        inode: u64,
        header_hash: &str,
    ) -> Result<()> {
        self.validate_live_source(device, inode, header_hash)?;

        return self.resume_at(&EventLogPosition {
            next_event_seq: position.next_event_seq,
            file_offset: position.file_offset,
            event_hash: position.event_hash.clone(),
            monotonic_ns: position.monotonic_ns,
        });
    }

    /// Position after the last record this cursor returned.
    pub fn position(&self) -> EventLogPosition {
        return EventLogPosition {
            next_event_seq: self.validation.expected_seq,
            file_offset: self.position_offset(),
            event_hash: self.validation.previous_hash.to_hex().to_string(),
            monotonic_ns: self.validation.previous_time,
        };
    }

    /// Seek a fresh cursor to a position taken from an earlier read of the same source.
    ///
    /// Only the suffix is validated, so the cursor can never finish as full-log verification.
    /// The caller must trust `position` and confirm the suffix with `validate_boundary`.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor has already read records or `position` precedes the bytes
    /// read so far, the position hash is not valid BLAKE3 hex, or seeking fails.
    pub fn resume_at(&mut self, position: &EventLogPosition) -> Result<()> {
        if self.validation.expected_seq != 0 || position.file_offset < self.encoded_bytes {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "invalid resume position",
            ));
        }

        let hash = Hash::from_hex(&position.event_hash).map_err(|source| {
            return Error::InvalidEventLog {
                path: self.path.clone(),
                line: self.line_number,
                reason: "invalid resume position hash".to_owned(),
                source: Some(Box::new(source)),
            };
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
    ///
    /// # Errors
    ///
    /// Returns an error when the opened file's device, inode, or header hash differs from the
    /// actor's, or the path no longer names the opened file at its read length.
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
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be inspected, no longer names the opened regular file,
    /// or is shorter than the bytes already read.
    pub fn validate_source_identity(&self) -> Result<()> {
        let current = symlink_metadata(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        let logical_length = self.file.len().map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;

        if !current.is_file()
            || current.dev() != self.opened_metadata.dev()
            || current.ino() != self.opened_metadata.ino()
            || logical_length < self.position_offset()
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
    ///
    /// # Errors
    ///
    /// Returns an error when the source identity changed, or the validated suffix's sequence, hash,
    /// offset, or timestamp differs from `boundary`.
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
    ///
    /// # Errors
    ///
    /// Returns an error when the source identity changed or the validated chain does not end at
    /// `next_event_seq` with `event_hash`.
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

                if self.validation.expected_seq == STREAM_EVENT_LOG_LIMITS.event_count {
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
            let capacity = self.snapshot_end.map_or(chunk.len(), |end| {
                return usize::try_from(end.saturating_sub(self.encoded_bytes))
                    .unwrap_or(usize::MAX)
                    .min(chunk.len());
            });
            let target = chunk.get_mut(..capacity).ok_or_else(|| {
                return invalid_log(
                    &self.path,
                    self.line_number,
                    "invalid snapshot read capacity",
                );
            })?;
            let count = match self.file.read(target) {
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

    /// Finish a finite snapshot, allowing an unfinished trailing record and later appends.
    ///
    /// This validates a prefix only and cannot establish session completion.
    ///
    /// # Errors
    /// Rejects replacement/truncation, suffix-only validation, and non-snapshot cursors.
    pub fn finish_snapshot(self) -> Result<Vec<PendingOperation>> {
        if self.snapshot_end.is_none() || !self.validation.validate_operations {
            return Err(invalid_log(
                &self.path,
                self.line_number,
                "cursor is not a full-prefix snapshot",
            ));
        }

        self.validate_source_identity()?;
        let mut pending: Vec<_> = self.validation.pending_operations.into_values().collect();
        pending.sort_by_key(|operation| return operation.intent_seq);

        return Ok(pending);
    }

    /// Require a complete file rather than an appendable live prefix.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor was resumed from a live suffix, the file ends in an
    /// unterminated record, or the file changed after it was opened.
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
            || self.opened_fingerprint
                != canonical_source_fingerprint(&self.path).map_err(|source| {
                    return Error::Filesystem {
                        path: self.path.clone(),
                        source,
                    };
                })?
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
        return Self::read_cursor(EventLogCursor::open(path)?, &mut check, false);
    }

    /// Validate a finite prefix of an appendable log without retaining its history.
    ///
    /// A partial trailing record is excluded. The returned summary is not a
    /// completion claim; callers must label artifacts derived from it as incomplete.
    ///
    /// # Errors
    /// Rejects invalid complete records and replacement or truncation of the source.
    pub fn snapshot(path: impl AsRef<Path>) -> Result<Self> {
        return Self::read_cursor(EventLogCursor::open_snapshot(path)?, || return Ok(()), true);
    }

    fn read_cursor(
        mut cursor: EventLogCursor,
        mut check: impl FnMut() -> Result<()>,
        snapshot: bool,
    ) -> Result<Self> {
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

        summary.pending_operations =
            if snapshot { cursor.finish_snapshot()? } else { cursor.finish()? };

        return Ok(summary);
    }
}

fn parse_event_log(path: &Path, event_limit: Option<u64>) -> Result<EventLog> {
    return parse_event_log_with_limits(path, event_limit, DEFAULT_EVENT_LOG_LIMITS);
}

pub(super) fn parse_event_log_with_limits(
    path: &Path,
    event_limit: Option<u64>,
    limits: EventLogLimits,
) -> Result<EventLog> {
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

    let file = CanonicalLogFile::open(path).map_err(|source| {
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
        seen_operations: IdentifierSet::default(),
        pending_operations: HashMap::new(),
        validate_operations: true,
    };

    let mut events: Vec<EventRecord> = Vec::new();
    let mut decoded_payload_bytes = 0_u64;
    let mut line_number = 2_u64;

    loop {
        if event_limit == Some(u64::try_from(events.len()).unwrap_or(u64::MAX)) {
            break;
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

                break;
            }
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

            return Err(error);
        }

        if u64::try_from(events.len()).unwrap_or(u64::MAX) >= limits.event_count {
            let error = invalid_log(
                path,
                line_number,
                format!("event log exceeds {} event limit", limits.event_count),
            );

            return Err(error);
        }

        {
            let record = validation.validate_record(path, line_number, &line)?;
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

                return Err(error);
            }

            events.push(record);
        }

        line_number = line_number.saturating_add(1);
    }

    let mut pending_operations: Vec<PendingOperation> =
        validation.pending_operations.into_values().collect();
    pending_operations.sort_by_key(|operation| return operation.intent_seq);

    return Ok(EventLog {
        header,
        events,
        pending_operations,
        source_path: Some(path.to_path_buf()),
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

fn recover_stream(source: &Path, recovered: &Path) -> Result<Option<RecoveryIssue>> {
    validate_parent_directory(recovered)?;

    let input =
        CanonicalLogFile::open(source).map_err(|error| return Error::filesystem(source, error))?;
    let mut reader = BufReader::new(input);
    let header_bytes = read_line(&mut reader, source, 1)?
        .ok_or_else(|| return invalid_log(source, 1, "missing header"))?;
    let header: EventLogHeader = parse_json_line(source, 1, &header_bytes)?;
    validate_header(source, &header)?;

    let mut validation = LogValidation {
        previous_hash: blake3::hash(&header_bytes),
        previous_time: 0,
        expected_seq: 0,
        seen_operations: IdentifierSet::default(),
        pending_operations: HashMap::new(),
        validate_operations: true,
    };

    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_FILE_MODE)
        .open(recovered)
        .map_err(|error| return Error::filesystem(recovered, error))?;
    let storage = SegmentWriter::new(recovered, output)
        .map_err(|error| return Error::filesystem(recovered, error))?;
    let mut writer = BufWriter::with_capacity(128 * 1024, storage);
    writer
        .write_all(&header_bytes)
        .and_then(|()| return writer.write_all(b"\n"))
        .map_err(|error| return Error::filesystem(recovered, error))?;

    let mut line_number = 2_u64;
    let issue = loop {
        let line = match read_line(&mut reader, source, line_number) {
            Ok(Some(line)) => line,
            Ok(None) => break None,
            Err(error) => break Some(recovery_issue(&error, line_number)),
        };

        if let Err(error) = validation.validate_record(source, line_number, &line) {
            break Some(recovery_issue(&error, line_number));
        }

        writer
            .write_all(&line)
            .and_then(|()| return writer.write_all(b"\n"))
            .map_err(|error| return Error::filesystem(recovered, error))?;

        line_number = line_number
            .checked_add(1)
            .ok_or_else(|| return invalid_log(source, line_number, "line number overflow"))?;
    };
    writer
        .flush()
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|error| return Error::filesystem(recovered, error))?;

    sync_parent_directory(recovered)?;

    return Ok(issue);
}
