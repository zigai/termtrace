mod operations;
mod reader;
mod segments;
mod types;
mod validation;
mod writer;

#[cfg(test)]
mod tests;

pub use reader::EventLogCursor;
pub use segments::{canonical_source_fingerprint, CanonicalLogFile};
pub use termtrace_model::execution::event_log::{
    CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord, OperationKind,
    PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport,
};
pub use types::{EventLog, EventLogSummary, SessionClock};
pub use writer::EventLogWriter;
