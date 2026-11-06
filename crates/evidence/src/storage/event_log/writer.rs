use super::{
    model::SessionClock,
    segments::SegmentWriter,
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
    io::{self, Write},
    mem::take,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::event::{Event, EventLogHeader},
    execution::event_log::{CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord},
    protocol::control::LiveEventCursor,
};

const RECENT_CURSOR_LIMIT: usize = 4096;
const EVENT_LOG_BUFFER_BYTES: usize = 128 * 1024;
// Bounds encoded events queued for storage to about 4 MiB before appends wait for it.
const STORAGE_QUEUE_LIMIT: usize = 32;

#[derive(Clone, Copy, Debug)]
struct WriterCursor {
    next_event_seq: u64,
    file_offset: u64,
    hash: Hash,
    monotonic_ns: u64,
    output_bytes: u64,
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

    fn boundary(self) -> CanonicalBoundary {
        return CanonicalBoundary {
            next_event_seq: self.next_event_seq,
            event_hash: self.hash.to_hex().to_string(),
            output_bytes: self.output_bytes,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Commit {
    Defer,
    Request,
    Await,
}

#[derive(Debug)]
enum StorageWork {
    Write(Vec<u8>),
    Sync {
        target: WriterCursor,
        pending_since: Option<Instant>,
    },
    Close,
}

#[derive(Debug)]
struct SyncState {
    durable: WriterCursor,
    sync_count: u64,
    maximum_observed_uncommitted_ns: u64,
    failed: bool,
    failure: Option<io::Error>,
}

#[derive(Debug)]
struct SyncShared {
    state: Mutex<SyncState>,
    completed: Condvar,
}

impl SyncShared {
    fn lock(&self) -> MutexGuard<'_, SyncState> {
        return self.state.lock().unwrap_or_else(PoisonError::into_inner);
    }

    fn complete(&self, target: WriterCursor, pending_since: Option<Instant>) {
        let mut state = self.lock();

        if target.next_event_seq >= state.durable.next_event_seq {
            state.durable = target;
        }

        if let Some(started) = pending_since {
            state.maximum_observed_uncommitted_ns = state
                .maximum_observed_uncommitted_ns
                .max(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }

        state.sync_count = state.sync_count.saturating_add(1);
        drop(state);

        self.completed.notify_all();
    }

    fn fail(&self, source: io::Error) {
        let mut state = self.lock();
        state.failed = true;
        state.failure = Some(source);
        drop(state);

        self.completed.notify_all();
    }
}

// Appending writes and data synchronization run on their own thread: on a busy filesystem
// either can block for seconds, and the recorder loop that timestamps events must not wait.
fn run_storage(
    mut storage: SegmentWriter,
    work: &mpsc::Receiver<StorageWork>,
    shared: &SyncShared,
) {
    while let Ok(next) = work.recv() {
        let (mut target, mut pending_since) = match next {
            StorageWork::Write(bytes) => {
                if let Err(source) = storage.write_all(&bytes) {
                    return shared.fail(source);
                }

                continue;
            }
            StorageWork::Sync {
                target,
                pending_since,
            } => (target, pending_since),
            StorageWork::Close => return,
        };

        let mut close = false;

        // Requests already queued share one synchronization; the drain is bounded so a
        // continuously writing recorder cannot postpone it indefinitely.
        for queued in work.try_iter().take(STORAGE_QUEUE_LIMIT) {
            match queued {
                StorageWork::Write(bytes) => {
                    if let Err(source) = storage.write_all(&bytes) {
                        return shared.fail(source);
                    }
                }
                StorageWork::Sync {
                    target: newer,
                    pending_since: newer_since,
                } => {
                    target = newer;
                    pending_since = pending_since.or(newer_since);
                }
                StorageWork::Close => {
                    close = true;

                    break;
                }
            }
        }

        if let Err(source) = storage.sync_data() {
            return shared.fail(source);
        }

        shared.complete(target, pending_since);

        if close {
            return;
        }
    }
}

/// Append-only writer for a hash-chained canonical event log.
///
/// Encoded events are written and data-synchronized on a dedicated storage thread. Bounded
/// durability's time threshold only requests synchronization, so event timestamps never wait
/// for storage; every other synchronization, including the byte threshold, still blocks the
/// append until it completes. Dropping the writer waits for queued events to reach the file.
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    pending: Vec<u8>,
    clock: SessionClock,
    next_seq: u64,
    previous_hash: Hash,
    observed_output_bytes: u64,
    durability_policy: DurabilityPolicy,
    pending_since: Option<Instant>,
    // A requested synchronization the storage thread has not completed: when the owner last
    // looked for its failure, and the event count it makes durable.
    sync_in_flight: Option<(Instant, u64)>,
    maximum_observed_uncommitted_bytes: u64,
    poisoned: bool,
    recent_cursors: VecDeque<WriterCursor>,
    observed_cursor: WriterCursor,
    // The durable cursor as the owner last adopted it, so boundaries read within one owner
    // step agree even while the storage thread completes synchronizations.
    durable_cursor: WriterCursor,
    storage: mpsc::SyncSender<StorageWork>,
    storage_thread: Option<JoinHandle<()>>,
    sync: Arc<SyncShared>,
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
        let durable = self.durable_cursor;
        let start = if next_event_seq == durable.next_event_seq {
            Some(durable.projection())
        } else {
            self.recent_cursors
                .iter()
                .find(|cursor| {
                    return cursor.next_event_seq == next_event_seq
                        && next_event_seq <= durable.next_event_seq;
                })
                .copied()
                .map(WriterCursor::projection)
        };

        return (start, durable.projection());
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
        return self.observed_cursor.boundary();
    }

    /// Latest event boundary known to be data-synchronized.
    pub fn durable_boundary(&self) -> CanonicalBoundary {
        return self.durable_cursor.boundary();
    }

    /// Durability measurements accumulated by this writer.
    pub fn durability_record(&self) -> DurabilityRecord {
        let state = self.sync.lock();

        return DurabilityRecord {
            policy: self.durability_policy.clone(),
            maximum_observed_uncommitted_bytes: self.maximum_observed_uncommitted_bytes,
            maximum_observed_uncommitted_ns: state.maximum_observed_uncommitted_ns,
            sync_count: state.sync_count,
            final_sync_completed: self.durable_cursor.next_event_seq == self.next_seq
                && self.durable_cursor.hash == self.previous_hash
                && self.durable_cursor.output_bytes == self.observed_output_bytes,
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
    /// Returns an error when the policy or header is invalid, creation cannot be synced, or
    /// the synchronization thread cannot start.
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

        let mut storage = storage;
        storage
            .write_all(&header_bytes)
            .and_then(|()| return storage.write_all(b"\n"))
            .and_then(|()| return storage.sync_data())
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;

        sync_parent_directory(&path)?;

        let metadata = storage.metadata().map_err(|source| {
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
            output_bytes: 0,
        };

        let mut recent_cursors = VecDeque::with_capacity(RECENT_CURSOR_LIMIT);
        recent_cursors.push_back(initial_cursor);

        let sync = Arc::new(SyncShared {
            state: Mutex::new(SyncState {
                durable: initial_cursor,
                sync_count: 0,
                maximum_observed_uncommitted_ns: 0,
                failed: false,
                failure: None,
            }),
            completed: Condvar::new(),
        });

        let (storage_work, work) = mpsc::sync_channel(STORAGE_QUEUE_LIMIT);
        let shared = Arc::clone(&sync);

        let storage_thread = thread::Builder::new()
            .name("termtrace-event-storage".to_owned())
            .spawn(move || return run_storage(storage, &work, &shared))
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;

        return Ok(Self {
            path,
            pending: Vec::with_capacity(EVENT_LOG_BUFFER_BYTES),
            clock,
            next_seq: 0,
            previous_hash,
            observed_output_bytes: 0,
            durability_policy,
            pending_since: None,
            sync_in_flight: None,
            maximum_observed_uncommitted_bytes: 0,
            poisoned: false,
            recent_cursors,
            observed_cursor: initial_cursor,
            durable_cursor: initial_cursor,
            storage: storage_work,
            storage_thread: Some(storage_thread),
            sync,
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

    /// Append a non-output event for a later synchronization to make durable.
    ///
    /// Strict durability synchronizes it at once and bounded durability within its interval;
    /// throughput durability waits for the next synchronized append.
    ///
    /// # Errors
    ///
    /// Returns an error for an output event or when persistence fails.
    pub fn append_deferred(&mut self, event: Event) -> Result<u64> {
        if matches!(event, Event::Output { .. }) {
            return Err(invalid_log(
                &self.path,
                self.next_seq.saturating_add(2),
                "append_deferred requires a non-output event",
            ));
        }

        let commit = if self.durability_policy.mode == DurabilityMode::Strict {
            Commit::Await
        } else {
            Commit::Defer
        };

        return self.append_at_observation(self.clock.elapsed_ns(), None, event, commit);
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
    /// The observation and append times are both retained. Bounded durability blocks once the
    /// byte threshold is reached and only requests synchronization at the time threshold.
    ///
    /// # Errors
    ///
    /// Returns an error when `event` is not an output event or cannot be persisted.
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
        let commit = match self.durability_policy.mode {
            DurabilityMode::Strict => Commit::Await,
            DurabilityMode::Throughput => Commit::Defer,
            DurabilityMode::Bounded => {
                let maximum_bytes = self
                    .durability_policy
                    .maximum_uncommitted_bytes
                    .unwrap_or(u64::MAX);

                let maximum_interval = self
                    .durability_policy
                    .maximum_interval_ms
                    .map_or(Duration::MAX, Duration::from_millis);

                if self.uncommitted_output_bytes().saturating_add(output_bytes) >= maximum_bytes {
                    Commit::Await
                } else if self
                    .pending_since
                    .is_some_and(|started| return started.elapsed() >= maximum_interval)
                {
                    Commit::Request
                } else {
                    Commit::Defer
                }
            }
        };

        let observation = observed.map(|instant| return self.clock.observed_ns(instant));

        return self.append_at_observation(self.clock.elapsed_ns(), observation, event, commit);
    }

    /// Adopt synchronizations the storage thread completed since the last call.
    ///
    /// The durable boundary, follow cursors, and durability record change only here and when
    /// an append waits for durability, so values read between calls are mutually consistent.
    pub fn observe_durable(&mut self) {
        self.durable_cursor = self.sync.lock().durable;

        if self.sync_in_flight.is_some_and(|(_, target)| {
            return self.durable_cursor.next_event_seq >= target;
        }) {
            self.sync_in_flight = None;
        }
    }

    /// When pending bounded-durability events reach their time threshold, or when a requested
    /// synchronization still running should next be checked for failure.
    #[must_use]
    pub fn flush_deadline(&self) -> Option<Instant> {
        if self.durability_policy.mode != DurabilityMode::Bounded {
            return None;
        }

        let interval = Duration::from_millis(self.durability_policy.maximum_interval_ms?);

        return [
            self.pending_since,
            self.sync_in_flight.map(|(checked, _)| return checked),
        ]
        .into_iter()
        .flatten()
        .min()?
        .checked_add(interval);
    }

    /// Request synchronization of pending bounded-durability events whose time threshold has
    /// elapsed, without waiting for it to complete.
    ///
    /// Returns whether synchronization was requested.
    ///
    /// # Errors
    ///
    /// Returns an error when an earlier write or synchronization failed, poisoning the writer.
    pub fn flush_due(&mut self) -> Result<bool> {
        self.ensure_writable()?;
        if self
            .flush_deadline()
            .is_none_or(|deadline| return Instant::now() < deadline)
        {
            return Ok(false);
        }

        let interval = self
            .durability_policy
            .maximum_interval_ms
            .map_or(Duration::MAX, Duration::from_millis);

        if self
            .pending_since
            .is_none_or(|started| return started.elapsed() < interval)
        {
            if let Some((checked, _)) = &mut self.sync_in_flight {
                *checked = Instant::now();
            }

            return Ok(false);
        }

        self.request_sync(self.observed_cursor)?;

        return Ok(true);
    }

    pub(super) fn append_at(&mut self, monotonic_ns: u64, event: Event) -> Result<u64> {
        return self.append_at_observation(monotonic_ns, None, event, Commit::Await);
    }

    #[cfg(test)]
    pub(super) fn await_durable(&mut self) -> Result<()> {
        return self.wait_for(self.next_seq);
    }

    #[cfg(test)]
    pub(super) fn inject_storage_failure(&self, source: io::Error) {
        self.sync.fail(source);
    }

    fn uncommitted_output_bytes(&self) -> u64 {
        return self
            .observed_output_bytes
            .saturating_sub(self.sync.lock().durable.output_bytes);
    }

    fn ensure_writable(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(Error::EventLogPoisoned {
                path: self.path.clone(),
            });
        }

        let mut state = self.sync.lock();
        if !state.failed {
            return Ok(());
        }

        let failure = state.failure.take();
        drop(state);
        self.poisoned = true;

        return Err(failure.map_or_else(
            || {
                return Error::EventLogPoisoned {
                    path: self.path.clone(),
                };
            },
            |source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            },
        ));
    }

    fn send(&mut self, work: StorageWork) -> Result<()> {
        if self.storage.send(work).is_err() {
            return Err(self.storage_failure());
        }
        return Ok(());
    }

    fn storage_failure(&mut self) -> Error {
        let failure = self.ensure_writable().err();
        self.poisoned = true;

        return failure.unwrap_or_else(|| {
            return Error::Filesystem {
                path: self.path.clone(),
                source: io::Error::other("event-log storage thread stopped"),
            };
        });
    }

    fn flush_pending(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let bytes = take(&mut self.pending);
        self.pending.reserve(EVENT_LOG_BUFFER_BYTES);

        return self.send(StorageWork::Write(bytes));
    }

    fn request_sync(&mut self, target: WriterCursor) -> Result<()> {
        self.flush_pending()?;
        let pending_since = self.pending_since.take();
        self.sync_in_flight = Some((Instant::now(), target.next_event_seq));

        return self.send(StorageWork::Sync {
            target,
            pending_since,
        });
    }

    fn wait_for(&mut self, next_event_seq: u64) -> Result<()> {
        let state = self
            .sync
            .completed
            .wait_while(self.sync.lock(), |state| {
                return !state.failed && state.durable.next_event_seq < next_event_seq;
            })
            .unwrap_or_else(PoisonError::into_inner);

        let failed = state.failed;
        let durable = state.durable;

        drop(state);

        if failed {
            return self.ensure_writable();
        }

        self.durable_cursor = durable;

        return Ok(());
    }

    fn append_at_observation(
        &mut self,
        monotonic_ns: u64,
        observed_monotonic_ns: Option<u64>,
        event: Event,
        commit: Commit,
    ) -> Result<u64> {
        if observed_monotonic_ns.is_some_and(|observed| return observed > monotonic_ns) {
            return Err(invalid_log(
                &self.path,
                self.next_seq + 2,
                "observation is after append time",
            ));
        }

        self.ensure_writable()?;
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

        self.pending.extend_from_slice(b"{\"seq\":");
        self.pending.extend_from_slice(seq_str.as_bytes());
        self.pending.extend_from_slice(b",\"monotonic_ns\":");
        self.pending.extend_from_slice(mono_str.as_bytes());

        if let Some(obs) = obs_str {
            self.pending
                .extend_from_slice(b",\"observed_monotonic_ns\":");
            self.pending.extend_from_slice(obs.as_bytes());
        }

        self.pending.extend_from_slice(b",\"prev_hash\":\"");
        self.pending
            .extend_from_slice(previous_hash_text.as_bytes());
        self.pending.extend_from_slice(b"\",\"hash\":\"");
        self.pending.extend_from_slice(hash_text.as_bytes());
        self.pending.extend_from_slice(b"\",\"event\":");
        self.pending.extend_from_slice(&self.event_scratch);
        self.pending.extend_from_slice(b"}\n");

        let cursor = WriterCursor {
            next_event_seq: next_seq,
            file_offset: self.observed_cursor.file_offset + encoded_bytes,
            hash,
            monotonic_ns,
            output_bytes: next_output_bytes,
        };

        if output_bytes > 0 {
            let uncommitted =
                next_output_bytes.saturating_sub(self.sync.lock().durable.output_bytes);
            self.maximum_observed_uncommitted_bytes =
                self.maximum_observed_uncommitted_bytes.max(uncommitted);
        }

        match commit {
            Commit::Defer if self.pending.len() >= EVENT_LOG_BUFFER_BYTES => {
                self.flush_pending()?;
            }
            Commit::Defer => {}
            Commit::Request => self.request_sync(cursor)?,
            Commit::Await => {
                self.request_sync(cursor)?;
                self.wait_for(next_seq)?;
            }
        }

        self.next_seq = next_seq;
        self.previous_hash = hash;
        self.observed_output_bytes = next_output_bytes;
        self.observed_cursor = cursor;

        if self.recent_cursors.len() == RECENT_CURSOR_LIMIT {
            self.recent_cursors.pop_front();
        }

        self.recent_cursors.push_back(cursor);

        if commit == Commit::Defer && self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }

        return Ok(seq);
    }
}

impl Drop for EventLogWriter {
    fn drop(&mut self) {
        // Readers may open the log as soon as the writer is gone, so queued events must
        // reach the file first; a stopped storage thread has already reported its failure.
        let _flushed = self.flush_pending();
        let _closed = self.storage.send(StorageWork::Close);

        if let Some(storage_thread) = self.storage_thread.take() {
            let _joined = storage_thread.join();
        }
    }
}
