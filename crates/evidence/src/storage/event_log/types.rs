use crate::storage::{Error, Result};
use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use termtrace_model::{
    execution::event::{Event, EventLogHeader, EventRecord},
    execution::event_log::PendingOperation,
};

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

    /// Convert a collector observation into this session's monotonic timebase.
    pub fn observed_ns(&self, observed: Instant) -> u64 {
        return u64::try_from(
            observed
                .saturating_duration_since(self.monotonic_origin)
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
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

/// Bounded content-validation result; output payloads are not retained.
#[derive(Clone, Debug)]
pub struct EventLogSummary {
    /// Immutable header carrying the session identity, initial geometry, and manifest hash.
    pub header: EventLogHeader,
    /// Number of canonical event records, excluding the header.
    pub event_count: u64,
    /// Total decoded payload bytes in canonical [`Event::Output`] records.
    pub output_bytes: u64,
    /// Final canonical record, or `None` when there are no events.
    /// This is not necessarily an [`Event::Exit`]; completion validation checks its kind.
    pub terminal: Option<EventRecord>,
    /// Mutation intents without a matching result at the end of the log.
    pub pending_operations: Vec<PendingOperation>,
    /// Presence of canonical [`Event::HostError`] resource-limit triggers in the log.
    /// Entries correspond to `output_budget_exceeded`, `max_duration_exceeded`, and
    /// `idle_timeout_exceeded`, in that order.
    pub resource_triggers: [bool; 3],
}

impl From<&EventLog> for EventLogSummary {
    fn from(log: &EventLog) -> Self {
        let mut summary = Self {
            header: log.header.clone(),
            event_count: u64::try_from(log.events.len()).unwrap_or(u64::MAX),
            output_bytes: 0,
            terminal: log.events.last().cloned(),
            pending_operations: log.pending_operations.clone(),
            resource_triggers: [false; 3],
        };

        for record in &log.events {
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
        }

        return summary;
    }
}
