use super::super::*;

/// Canonical child-output history operations.
pub struct Output<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Output<'_> {
    /// Wait for exact bytes ever emitted by the canonical child stream.
    pub fn wait_for(&self, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return Ok(wait_for_output(&self.session.path, bytes, timeout)?);
    }
}
