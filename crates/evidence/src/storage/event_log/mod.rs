mod operations;
mod reader;
mod types;
mod validation;
mod writer;

#[cfg(test)]
mod tests;

pub use reader::EventLogCursor;
pub use termtrace_model::execution::event_log::{
    CanonicalBoundary, DurabilityMode, DurabilityPolicy, DurabilityRecord, OperationKind,
    PendingOperation, PendingOperationReport, RecoveryIssue, RecoveryReport,
};
pub use types::{EventLog, SessionClock};
pub use writer::EventLogWriter;
