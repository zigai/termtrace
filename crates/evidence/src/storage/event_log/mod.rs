mod model;
mod operations;
mod reader;
mod segments;
#[cfg(test)]
mod tests;
mod validation;
mod writer;

pub use model::{EventLog, EventLogSummary, SessionClock};
pub use reader::EventLogCursor;
pub use segments::{canonical_source_fingerprint, CanonicalLogFile};
pub use termtrace_model::execution::event_log::{
    CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord, OperationKind,
    PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport,
};
pub use writer::EventLogWriter;
