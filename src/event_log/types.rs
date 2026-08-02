use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    event::{EventLogHeader, EventRecord},
};

/// Output persistence behavior selected for one canonical recording.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityMode {
    /// Synchronize every PTY output event before processing continues.
    Strict,
    /// Synchronize after a configured output-byte or elapsed-time threshold.
    Bounded,
    /// Buffer PTY output until a mutation, lifecycle, or completion boundary.
    Throughput,
}

/// Chain-bound configuration for canonical output persistence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DurabilityPolicy {
    /// Selected persistence behavior.
    pub mode: DurabilityMode,
    /// Maximum uncommitted interval for bounded mode.
    pub maximum_interval_ms: Option<u64>,
    /// Maximum uncommitted PTY bytes for bounded mode.
    pub maximum_uncommitted_bytes: Option<u64>,
}

impl Default for DurabilityPolicy {
    fn default() -> Self {
        return Self {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(50),
            maximum_uncommitted_bytes: Some(64 * 1024),
        };
    }
}

impl DurabilityPolicy {
    /// Validate that thresholds are present only for bounded mode and are nonzero.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode and threshold fields disagree.
    pub fn validate(&self) -> Result<()> {
        let valid = match self.mode {
            DurabilityMode::Strict | DurabilityMode::Throughput => {
                self.maximum_interval_ms.is_none() && self.maximum_uncommitted_bytes.is_none()
            }
            DurabilityMode::Bounded => {
                self.maximum_interval_ms
                    .is_some_and(|value| return value > 0)
                    && self
                        .maximum_uncommitted_bytes
                        .is_some_and(|value| return value > 0)
            }
        };
        if !valid {
            return Err(Error::InvalidReplay {
                reason: "durability policy thresholds are invalid for the selected mode".to_owned(),
            });
        }
        return Ok(());
    }
}

/// Recorder-observed durability outcomes bound into completion evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DurabilityRecord {
    /// Chain-bound configured policy.
    pub policy: DurabilityPolicy,
    /// Largest PTY byte count pending synchronization.
    pub maximum_observed_uncommitted_bytes: u64,
    /// Largest measured interval with output pending synchronization.
    pub maximum_observed_uncommitted_ns: u64,
    /// Number of successful event-log data synchronizations after header creation.
    pub sync_count: u64,
    /// Whether the final event boundary completed file synchronization.
    pub final_sync_completed: bool,
}

/// Actor-consistent position in the canonical event stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalBoundary {
    /// Sequence number that will be assigned to the next event.
    pub next_event_seq: u64,
    /// Hash of the preceding event, or the event header at sequence zero.
    pub event_hash: String,
    /// PTY output bytes included through this boundary.
    pub output_bytes: u64,
}

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

    /// Nanoseconds elapsed from the monotonic origin, saturating at `u64::MAX`.
    pub fn elapsed_ns(&self) -> u64 {
        return u64::try_from(self.monotonic_origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
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

/// First invalid tail encountered during prefix recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct RecoveryIssue {
    /// One-based source line where validation failed.
    pub line: u64,
    /// Stable human-readable validation failure.
    pub reason: String,
}

/// Evidence produced by non-destructive event-log prefix recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct RecoveryReport {
    /// Original source path.
    pub source: PathBuf,
    /// New owner-only recovered path.
    pub recovered: PathBuf,
    /// Number of integrity-checked events retained.
    pub valid_events: u64,
    /// Invalid tail, if one was discarded.
    pub discarded_tail: Option<RecoveryIssue>,
    /// Operations with a durable intent but uncertain result.
    pub pending_operations: Vec<PendingOperationReport>,
}

/// Serializable uncertain-operation summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct PendingOperationReport {
    /// Operation identifier.
    pub operation_id: Uuid,
    /// Stable operation kind.
    pub kind: String,
    /// Intent sequence number.
    pub intent_seq: u64,
    /// Intended input byte count.
    pub expected_bytes: Option<u64>,
}
