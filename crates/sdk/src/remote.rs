//! Typed live-session control without linking the recorder or native renderer.

use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_control::{ExchangeTiming, Result};
use termtrace_model::protocol::control::{
    ControlCommand, ControlData, ControlRequest, ControlResponse,
};

/// Lightweight connection configuration for a canonical live session.
///
/// The recorder remains in the installed `termtrace` process. Each request opens
/// an authenticated local connection, so handles can be cloned across threads.
#[derive(Clone, Debug)]
pub struct Client {
    session: PathBuf,
}

impl Client {
    /// Select the session advertised to a recorded child through `TERMTRACE_SESSION`.
    ///
    /// Returns `None` when this process has no recorder-provided session path.
    #[must_use]
    pub fn from_environment() -> Option<Self> {
        return env::var_os("TERMTRACE_SESSION").map(Self::new);
    }

    /// Select a canonical session directory without performing I/O.
    #[must_use]
    pub fn new(session: impl Into<PathBuf>) -> Self {
        return Self {
            session: session.into(),
        };
    }

    /// Canonical session directory selected by this handle.
    #[must_use]
    pub fn session_path(&self) -> &Path {
        return &self.session;
    }

    /// Send a typed command with the command's normal operation deadline.
    ///
    /// # Errors
    ///
    /// Returns the control error when the session cannot be reached, the exchange fails or times
    /// out, or the actor rejects the command.
    pub fn request(&self, command: ControlCommand) -> Result<ControlData> {
        return termtrace_control::request(&self.session, command);
    }

    /// Send an existing request, retaining structured actor rejection details.
    ///
    /// Preserve the same request ID and payload when retrying uncertain delivery.
    ///
    /// # Errors
    ///
    /// Returns the control error when the session cannot be reached or the exchange fails or times
    /// out; actor rejections are returned in the response.
    pub fn send(&self, request: &ControlRequest) -> Result<ControlResponse> {
        return termtrace_control::send_request_response(&self.session, request);
    }

    /// Send with one total deadline shared across connect, request, and response I/O.
    ///
    /// # Errors
    ///
    /// Returns the control error when `timeout` cannot form a deadline, the session cannot be
    /// reached, or the exchange fails or exceeds the deadline; actor rejections are returned in the
    /// response.
    pub fn send_with_timeout(
        &self,
        request: &ControlRequest,
        timeout: Duration,
    ) -> Result<ControlResponse> {
        return termtrace_control::exchange(
            &self.session,
            request,
            ExchangeTiming::deadline(timeout)?,
        );
    }

    /// Check whether the recorder's authenticated control endpoint is reachable.
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        return termtrace_control::control_endpoint_is_reachable(&self.session);
    }
}
