//! Typed live-session control without linking the recorder or native renderer.

use crate::{
    protocol::{self, ExchangeTiming},
    Error, Result,
};
use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};
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
    /// Connect to the session advertised to a recorded child through `TERMTRACE_SESSION`.
    ///
    /// # Errors
    /// Returns an error when this process has no recorder-provided session path.
    pub fn from_environment() -> Result<Self> {
        let session = env::var_os("TERMTRACE_SESSION").ok_or_else(|| {
            return Error::InvalidOperation {
                reason: "TERMTRACE_SESSION is not set".to_owned(),
            };
        })?;

        return Ok(Self::new(session));
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
    pub fn request(&self, command: ControlCommand) -> Result<ControlData> {
        return protocol::request(&self.session, command);
    }

    /// Send an existing request, retaining structured actor rejection details.
    ///
    /// Preserve the same request ID and payload when retrying uncertain delivery.
    pub fn send(&self, request: &ControlRequest) -> Result<ControlResponse> {
        return protocol::send_request_response(&self.session, request);
    }

    /// Send with one total deadline shared by request and response I/O.
    pub fn send_with_timeout(
        &self,
        request: &ControlRequest,
        timeout: Duration,
    ) -> Result<ControlResponse> {
        return protocol::exchange(&self.session, request, ExchangeTiming::deadline(timeout)?);
    }

    /// Check whether the recorder's authenticated control endpoint is reachable.
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        return protocol::control_endpoint_is_reachable(&self.session);
    }
}
