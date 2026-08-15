use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use termtrace_model::{
    execution::event::{EventLogHeader, EventRecord},
    execution::event_log::PendingOperation,
};

use crate::storage::{Error, Result};

/// Shared wall-clock and monotonic origins for one recording session.
#[derive(Clone, Debug)]
pub struct SessionClock {
    monotonic_origin: Instant,
    started_unix_ns: u128,
}

impl SessionClock {
    /// Capture new wall-clock and monotonic origins.
    ///
    /// # Errors
    ///
    /// Returns an error when the system clock is before the Unix epoch.
    pub fn now() -> Result<Self> {
        let started_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| return Error::ClockBeforeEpoch)?
            .as_nanos();
        return Ok(Self {
            monotonic_origin: Instant::now(),
            started_unix_ns,
        });
    }

    /// Wall-clock Unix timestamp corresponding to monotonic time zero.
    pub fn started_unix_ns(&self) -> u128 {
        return self.started_unix_ns;
    }

    /// Nanoseconds elapsed from the session's monotonic origin.
    pub fn elapsed_ns(&self) -> u64 {
        return u64::try_from(self.monotonic_origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
    }
}

/// Fully validated canonical event log loaded into memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventLog {
    /// Immutable event-log header.
    pub header: EventLogHeader,
    /// Ordered, hash-validated canonical events.
    pub events: Vec<EventRecord>,
    /// Intent records that have no matching result.
    pub pending_operations: Vec<PendingOperation>,
    /// Source file when the log was read from storage.
    pub source_path: Option<PathBuf>,
}
