//! Crash-resistant append-only persistence and strict event-log recovery.

mod operations;
mod reader;
mod types;
mod validation;
mod writer;

pub(crate) use reader::EventLogCursor;
pub use types::{
    CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord, EventLog, OperationKind,
    PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport, SessionClock,
};
pub use writer::EventLogWriter;

#[cfg(test)]
mod tests;
