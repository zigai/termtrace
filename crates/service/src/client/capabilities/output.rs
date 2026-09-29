use super::super::{wait_for_output, Duration, Result, Session, WaitOutcome};

/// Canonical child-output history operations.
pub struct Output<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Output<'_> {
    /// Wait for exact bytes ever emitted by the canonical child stream.
    ///
    /// # Errors
    /// Returns an error when `bytes` is empty, the event log is invalid, the child exits or the
    /// session completes before the bytes appear, or `timeout` elapses first.
    pub fn wait_for(&self, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return Ok(wait_for_output(self.session.path(), bytes, timeout)?);
    }
}
