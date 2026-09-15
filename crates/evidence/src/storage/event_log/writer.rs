use super::{
    segments::SegmentWriter,
    types::SessionClock,
    validation::{
        invalid_log, validate_event_geometry, validate_header, validate_parent_directory,
        OWNER_FILE_MODE,
    },
};
use crate::storage::{artifact::sync_parent_directory, Error, Result};
use blake3::Hash;
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::event::{Event, EventLogHeader},
    execution::event_log::{CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord},
    protocol::control::LiveEventCursor,
};

const RECENT_CURSOR_LIMIT: usize = 4096;
const EVENT_LOG_BUFFER_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug)]
struct WriterCursor {
    next_event_seq: u64,
    file_offset: u64,
    hash: Hash,
    monotonic_ns: u64,
}

impl WriterCursor {
    fn projection(self) -> LiveEventCursor {
        return LiveEventCursor {
            next_event_seq: self.next_event_seq,
            file_offset: self.file_offset,
            event_hash: self.hash.to_hex().to_string(),
            monotonic_ns: self.monotonic_ns,
        };
    }
}

/// Append-only writer for a hash-chained canonical event log.
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    writer: BufWriter<SegmentWriter>,
    clock: SessionClock,
    next_seq: u64,
    previous_hash: Hash,
    observed_output_bytes: u64,
    durable_next_seq: u64,
    durable_hash: Hash,
    durable_output_bytes: u64,
    durability_policy: DurabilityPolicy,
    uncommitted_output_bytes: u64,
    uncommitted_since: Option<Instant>,
    maximum_observed_uncommitted_bytes: u64,
    maximum_observed_uncommitted_ns: u64,
    sync_count: u64,
    poisoned: bool,
    recent_cursors: VecDeque<WriterCursor>,
    observed_cursor: WriterCursor,
    durable_cursor: WriterCursor,
    device: u64,
    inode: u64,
    header_hash: Hash,
    event_scratch: Vec<u8>,
}

impl EventLogWriter {
    /// Canonical event-log path owned by this writer.
    #[must_use]
    pub fn path(&self) -> &Path {
        return &self.path;
    }

    /// Writer descriptor identity and immutable header hash for live followers.
    pub fn source_identity(&self) -> (u64, u64, String) {
        return (
            self.device,
            self.inode,
            self.header_hash.to_hex().to_string(),
        );
    }

    /// Return the requested retained durable cursor and the latest durable cursor.
    pub fn follow_cursors(
        &self,
        next_event_seq: u64,
    ) -> (Option<LiveEventCursor>, LiveEventCursor) {
        let start = if next_event_seq == self.durable_cursor.next_event_seq {
            Some(self.durable_cursor.projection())
        } else {
            self.recent_cursors
                .iter()
                .find(|cursor| {
                    return cursor.next_event_seq == next_event_seq
                        && next_event_seq <= self.durable_cursor.next_event_seq;
                })
                .copied()
                .map(WriterCursor::projection)
        };

        return (start, self.durable_cursor.projection());
    }

    /// Nanoseconds elapsed since the session clock origin.
    pub fn elapsed_ns(&self) -> u64 {
        return self.clock.elapsed_ns();
    }

    /// Sequence number that will be assigned to the next event.
    pub fn next_seq(&self) -> u64 {
        return self.next_seq;
    }

    /// Hash of the latest observed event, or the header before the first event.
    pub fn current_hash(&self) -> String {
        return self.previous_hash.to_hex().to_string();
    }

    /// Latest event boundary accepted by the writer.
    pub fn observed_boundary(&self) -> CanonicalBoundary {
        return CanonicalBoundary {
            next_event_seq: self.next_seq,
            event_hash: self.previous_hash.to_hex().to_string(),
            output_bytes: self.observed_output_bytes,
        };
    }

    /// Latest event boundary known to be data-synchronized.
    pub fn durable_boundary(&self) -> CanonicalBoundary {
        return CanonicalBoundary {
            next_event_seq: self.durable_next_seq,
            event_hash: self.durable_hash.to_hex().to_string(),
            output_bytes: self.durable_output_bytes,
        };
    }

    /// Durability measurements accumulated by this writer.
    pub fn durability_record(&self) -> DurabilityRecord {
        return DurabilityRecord {
            policy: self.durability_policy.clone(),
            maximum_observed_uncommitted_bytes: self.maximum_observed_uncommitted_bytes,
            maximum_observed_uncommitted_ns: self.maximum_observed_uncommitted_ns,
            sync_count: self.sync_count,
            final_sync_completed: self.durable_next_seq == self.next_seq
                && self.durable_hash == self.previous_hash
                && self.durable_output_bytes == self.observed_output_bytes,
        };
    }

    /// Create a new event log using the default durability policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the header, path, or clock is invalid or creation cannot be synced.
    pub fn create(
        path: impl AsRef<Path>,
        header: &EventLogHeader,
        clock: SessionClock,
    ) -> Result<Self> {
        return Self::create_with_policy(path, header, clock, DurabilityPolicy::default());
    }

    /// Create a new event log with an explicit durability policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy or header is invalid or creation cannot be synced.
    pub fn create_with_policy(
        path: impl AsRef<Path>,
        header: &EventLogHeader,
        clock: SessionClock,
        durability_policy: DurabilityPolicy,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        validate_parent_directory(&path)?;
        validate_header(&path, header)?;

        durability_policy.validate()?;

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
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;

        let header_bytes = serde_json::to_vec(header).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let previous_hash = blake3::hash(&header_bytes);

        let storage = SegmentWriter::new(&path, file).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let mut writer = BufWriter::with_capacity(EVENT_LOG_BUFFER_BYTES, storage);
        writer
            .write_all(&header_bytes)
            .and_then(|()| return writer.write_all(b"\n"))
            .and_then(|()| return writer.flush())
            .and_then(|()| return writer.get_ref().sync_data())
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;

        sync_parent_directory(&path)?;

        let metadata = writer.get_ref().metadata().map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let initial_cursor = WriterCursor {
            next_event_seq: 0,
            file_offset: u64::try_from(header_bytes.len()).unwrap_or(u64::MAX) + 1,
            hash: previous_hash,
            monotonic_ns: 0,
        };

        let mut recent_cursors = VecDeque::with_capacity(RECENT_CURSOR_LIMIT);
        recent_cursors.push_back(initial_cursor);

        return Ok(Self {
            path,
            writer,
            clock,
            next_seq: 0,
            previous_hash,
            observed_output_bytes: 0,
            durable_next_seq: 0,
            durable_hash: previous_hash,
            durable_output_bytes: 0,
            durability_policy,
            uncommitted_output_bytes: 0,
            uncommitted_since: None,
            maximum_observed_uncommitted_bytes: 0,
            maximum_observed_uncommitted_ns: 0,
            sync_count: 0,
            poisoned: false,
            recent_cursors,
            observed_cursor: initial_cursor,
            durable_cursor: initial_cursor,
            device: metadata.dev(),
            inode: metadata.ino(),
            header_hash: previous_hash,
            event_scratch: Vec::with_capacity(64 * 1024),
        });
    }

    /// Append and synchronize a canonical event at the current session time.
    ///
    /// # Errors
    ///
    /// Returns an error when the event is invalid or cannot be persisted.
    pub fn append(&mut self, event: Event) -> Result<u64> {
        return self.append_at(self.clock.elapsed_ns(), event);
    }

    /// Append an output event according to the configured durability policy.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-output event or when persistence fails.
    pub fn append_output(&mut self, event: Event) -> Result<u64> {
        return self.append_observed_output(event, None);
    }

    /// Append output with the collector's read-completion timestamp.
    ///
    /// The observation and append times are both retained; the durability policy is unchanged.
    pub fn append_observed_output(
        &mut self,
        event: Event,
        observed: Option<Instant>,
    ) -> Result<u64> {
        let Event::Output { data } = &event else {
            return Err(invalid_log(
                &self.path,
                self.next_seq.saturating_add(2),
                "append_output requires an Output event",
            ));
        };
        let output_bytes = u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX);
        let durable = match self.durability_policy.mode {
            DurabilityMode::Strict => true,
            DurabilityMode::Throughput => false,
            DurabilityMode::Bounded => {
                let maximum_bytes = self
                    .durability_policy
                    .maximum_uncommitted_bytes
                    .unwrap_or(u64::MAX);

                let maximum_interval = self
                    .durability_policy
                    .maximum_interval_ms
                    .map_or(Duration::MAX, Duration::from_millis);

                self.uncommitted_output_bytes.saturating_add(output_bytes) >= maximum_bytes
                    || self
                        .uncommitted_since
                        .is_some_and(|started| return started.elapsed() >= maximum_interval)
            }
        };

        let observation = observed.map(|instant| return self.clock.observed_ns(instant));

        return self.append_at_observation(self.clock.elapsed_ns(), observation, event, durable);
    }

    /// Synchronize pending bounded-durability output when its time threshold has elapsed.
    ///
    /// Returns whether synchronization occurred.
    ///
    /// # Errors
    ///
    /// Returns an error when synchronization fails, poisoning the writer.
    pub fn flush_due(&mut self) -> Result<bool> {
        if self.durability_policy.mode != DurabilityMode::Bounded
            || self.uncommitted_since.is_none_or(|started| {
                return started.elapsed()
                    < Duration::from_millis(
                        self.durability_policy
                            .maximum_interval_ms
                            .unwrap_or(u64::MAX),
                    );
            })
        {
            return Ok(false);
        }

        self.synchronize_observed()?;

        return Ok(true);
    }

    pub(super) fn append_at(&mut self, monotonic_ns: u64, event: Event) -> Result<u64> {
        return self.append_at_policy(monotonic_ns, event, true);
    }

    fn append_at_policy(&mut self, monotonic_ns: u64, event: Event, durable: bool) -> Result<u64> {
        return self.append_at_observation(monotonic_ns, None, event, durable);
    }

    fn append_at_observation(
        &mut self,
        monotonic_ns: u64,
        observed_monotonic_ns: Option<u64>,
        event: Event,
        durable: bool,
    ) -> Result<u64> {
        if observed_monotonic_ns.is_some_and(|observed| return observed > monotonic_ns) {
            return Err(invalid_log(
                &self.path,
                self.next_seq + 2,
                "observation is after append time",
            ));
        }

        if self.poisoned {
            return Err(Error::EventLogPoisoned {
                path: self.path.clone(),
            });
        }

        let seq = self.next_seq;
        validate_event_geometry(&self.path, seq.saturating_add(2), &event)?;
        let next_seq = seq.checked_add(1).ok_or_else(|| {
            return invalid_log(&self.path, seq.saturating_add(2), "event sequence overflow");
        })?;

        self.event_scratch.clear();
        serde_json::to_writer(&mut self.event_scratch, &event).map_err(|source| {
            return Error::InvalidJson {
                path: self.path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let mut seq_buf = itoa::Buffer::new();
        let seq_str = seq_buf.format(seq);
        let mut mono_buf = itoa::Buffer::new();
        let mono_str = mono_buf.format(monotonic_ns);
        let mut obs_buf = itoa::Buffer::new();
        let obs_str = observed_monotonic_ns.map(|obs| return obs_buf.format(obs));
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.previous_hash.as_bytes());
        hasher.update(b"{\"seq\":");
        hasher.update(seq_str.as_bytes());
        hasher.update(b",\"monotonic_ns\":");
        hasher.update(mono_str.as_bytes());

        if let Some(obs) = obs_str {
            hasher.update(b",\"observed_monotonic_ns\":");
            hasher.update(obs.as_bytes());
        }

        hasher.update(b",\"event\":");
        hasher.update(&self.event_scratch);
        hasher.update(b"}");
        let hash = hasher.finalize();

        let output_bytes = match &event {
            Event::Output { data } => u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX),
            _ => 0,
        };

        let next_output_bytes = self
            .observed_output_bytes
            .checked_add(output_bytes)
            .ok_or_else(|| {
                return invalid_log(
                    &self.path,
                    seq.saturating_add(2),
                    "output byte count overflow",
                );
            })?;

        let previous_hash_text = self.previous_hash.to_hex();
        let hash_text = hash.to_hex();

        let mut encoded_len = 7
            + seq_str.len()
            + 16
            + mono_str.len()
            + 14
            + 64
            + 10
            + 64
            + 10
            + self.event_scratch.len()
            + 2;

        if let Some(obs) = obs_str {
            encoded_len += 25 + obs.len();
        }

        let encoded_bytes = u64::try_from(encoded_len).unwrap_or(u64::MAX);

        let write_result = (|| -> io::Result<()> {
            self.writer.write_all(b"{\"seq\":")?;
            self.writer.write_all(seq_str.as_bytes())?;
            self.writer.write_all(b",\"monotonic_ns\":")?;
            self.writer.write_all(mono_str.as_bytes())?;

            if let Some(obs) = obs_str {
                self.writer.write_all(b",\"observed_monotonic_ns\":")?;
                self.writer.write_all(obs.as_bytes())?;
            }

            self.writer.write_all(b",\"prev_hash\":\"")?;
            self.writer.write_all(previous_hash_text.as_bytes())?;
            self.writer.write_all(b"\",\"hash\":\"")?;
            self.writer.write_all(hash_text.as_bytes())?;
            self.writer.write_all(b"\",\"event\":")?;
            self.writer.write_all(&self.event_scratch)?;
            self.writer.write_all(b"}\n")?;

            return Ok(());
        })();

        let write_result = if durable {
            write_result
                .and_then(|()| return self.writer.flush())
                .and_then(|()| return self.writer.get_ref().sync_data())
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
        self.observed_output_bytes = next_output_bytes;
        self.observed_cursor = WriterCursor {
            next_event_seq: next_seq,
            file_offset: self.observed_cursor.file_offset + encoded_bytes,
            hash,
            monotonic_ns,
        };

        if self.recent_cursors.len() == RECENT_CURSOR_LIMIT {
            self.recent_cursors.pop_front();
        }

        self.recent_cursors.push_back(self.observed_cursor);

        if output_bytes > 0 {
            if self.uncommitted_since.is_none() {
                self.uncommitted_since = Some(Instant::now());
            }

            self.uncommitted_output_bytes =
                self.uncommitted_output_bytes.saturating_add(output_bytes);
            self.maximum_observed_uncommitted_bytes = self
                .maximum_observed_uncommitted_bytes
                .max(self.uncommitted_output_bytes);
        }

        if durable {
            self.durable_next_seq = next_seq;
            self.durable_hash = hash;
            self.durable_output_bytes = next_output_bytes;
            self.durable_cursor = self.observed_cursor;
            self.record_successful_sync();
        }

        return Ok(seq);
    }

    fn synchronize_observed(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(Error::EventLogPoisoned {
                path: self.path.clone(),
            });
        }

        if let Err(source) = self
            .writer
            .flush()
            .and_then(|()| return self.writer.get_ref().sync_data())
        {
            self.poisoned = true;

            return Err(Error::Filesystem {
                path: self.path.clone(),
                source,
            });
        }

        self.durable_next_seq = self.next_seq;
        self.durable_hash = self.previous_hash;
        self.durable_output_bytes = self.observed_output_bytes;
        self.durable_cursor = self.observed_cursor;
        self.record_successful_sync();

        return Ok(());
    }

    fn record_successful_sync(&mut self) {
        if let Some(started) = self.uncommitted_since.take() {
            self.maximum_observed_uncommitted_ns = self
                .maximum_observed_uncommitted_ns
                .max(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }

        self.uncommitted_output_bytes = 0;
        self.sync_count = self.sync_count.saturating_add(1);
    }
}
