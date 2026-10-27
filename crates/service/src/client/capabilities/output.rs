use super::super::{wait_for_output, Duration, Result, Session, WaitOutcome};
use crate::output::OutputService;
use termtrace_model::execution::paging::{ByteOutputPage, OutputPageCursor};

/// Canonical child-output history operations.
pub struct Output<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Output<'_> {
    /// Read exact source-bound PTY bytes with snapshot-relative lookahead.
    ///
    /// # Errors
    /// Rejects invalid cursors/limits and invalid canonical evidence.
    pub fn page(&self, cursor: Option<OutputPageCursor>, limit: usize) -> Result<ByteOutputPage> {
        return OutputService::page(self.session.path(), cursor, limit);
    }

    /// Wait for exact bytes ever emitted by the canonical child stream.
    ///
    /// # Errors
    /// Returns an error when `bytes` is empty, the event log is invalid, the child exits or the
    /// session completes before the bytes appear, or `timeout` elapses first.
    pub fn wait_for(&self, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return Ok(wait_for_output(self.session.path(), bytes, timeout)?);
    }
}
