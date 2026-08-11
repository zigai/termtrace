use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use blake3::Hash;

use crate::{
    Error, Result,
    artifact::sync_parent_directory,
    event::{Event, EventBody, EventLogHeader, EventRecord},
};

use super::{
    types::{CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord, SessionClock},
    validation::{
        OWNER_FILE_MODE, chained_hash, invalid_log, validate_event_geometry, validate_header,
        validate_parent_directory,
    },
};

/// Append-only event-log writer that durably commits every acknowledged event.
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    writer: BufWriter<File>,
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
}

impl EventLogWriter {
    /// Current nanoseconds on the session's monotonic clock.
    pub fn elapsed_ns(&self) -> u64 {
        return self.clock.elapsed_ns();
    }

    /// Sequence number that will be assigned to the next event.
    pub fn next_seq(&self) -> u64 {
        return self.next_seq;
    }

    /// Hex hash of the most recently appended record, or the header before events.
    pub fn current_hash(&self) -> String {
        return self.previous_hash.to_hex().to_string();
    }

    /// Latest actor-processed boundary, which may include buffered output.
    pub fn observed_boundary(&self) -> CanonicalBoundary {
        return CanonicalBoundary {
            next_event_seq: self.next_seq,
            event_hash: self.previous_hash.to_hex().to_string(),
            output_bytes: self.observed_output_bytes,
        };
    }

    /// Latest boundary known to have completed the configured file sync.
    pub fn durable_boundary(&self) -> CanonicalBoundary {
        return CanonicalBoundary {
            next_event_seq: self.durable_next_seq,
            event_hash: self.durable_hash.to_hex().to_string(),
            output_bytes: self.durable_output_bytes,
        };
    }

    /// Recorder-observed durability measurements through the latest sync.
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
    #[cfg(test)]
    pub(crate) fn poison_for_test(&mut self) {
        self.poisoned = true;
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
        return Self::create_with_policy(path, header, clock, DurabilityPolicy::default());
    }

    /// Create a new event log with an explicit chain-bound durability policy.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid policy or when secure durable creation fails.
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
        let mut writer = BufWriter::new(file);
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
        });
    }

    /// Append and durably synchronize an event before returning its sequence number.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, writing, flushing, synchronization,
    /// or sequence advancement fails.
    pub fn append(&mut self, event: Event) -> Result<u64> {
        return self.append_at(self.clock.elapsed_ns(), event);
    }

    /// Append one PTY output event according to the configured durability policy.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-output event or persistence failure.
    pub fn append_output(&mut self, event: Event) -> Result<u64> {
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
        return self.append_at_policy(self.clock.elapsed_ns(), event, durable);
    }

    /// Synchronize a bounded-policy prefix when its elapsed threshold is due.
    ///
    /// # Errors
    ///
    /// Returns an error when flushing or synchronization fails.
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
        if self.poisoned {
            return Err(Error::EventLogPoisoned {
                path: self.path.clone(),
            });
        }
        let seq = self.next_seq;
        validate_event_geometry(&self.path, seq.saturating_add(2), &event)?;
        let next_seq = seq.checked_add(1).ok_or_else(|| {
            return Error::InvalidEventLog {
                path: self.path.clone(),
                line: seq.saturating_add(2),
                reason: "event sequence overflow".to_owned(),
            };
        })?;
        let body = EventBody {
            seq,
            monotonic_ns,
            event: &event,
        };
        let body_bytes = serde_json::to_vec(&body).map_err(|source| {
            return Error::InvalidJson {
                path: self.path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let hash = chained_hash(self.previous_hash, &body_bytes);
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
        let record = EventRecord {
            seq,
            monotonic_ns,
            prev_hash: self.previous_hash.to_hex().to_string(),
            hash: hash.to_hex().to_string(),
            event,
        };
        let record_bytes = serde_json::to_vec(&record).map_err(|source| {
            return Error::InvalidJson {
                path: self.path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let write_result = self
            .writer
            .write_all(&record_bytes)
            .and_then(|()| return self.writer.write_all(b"\n"));
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
        self.record_successful_sync();
        return Ok(());
    }

    fn record_successful_sync(&mut self) {
        if let Some(started) = self.uncommitted_since.take() {
            let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.maximum_observed_uncommitted_ns =
                self.maximum_observed_uncommitted_ns.max(elapsed);
        }
        self.uncommitted_output_bytes = 0;
        self.sync_count = self.sync_count.saturating_add(1);
    }
}
