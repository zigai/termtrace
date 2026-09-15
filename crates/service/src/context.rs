//! Context and deadline management across service operations.

use crate::error::{Error, Result};
use std::{
    io::{Error as IoError, ErrorKind as IoErrorKind},
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Operation context tracking request identity, total deadlines, and cancellation.
#[derive(Clone, Debug)]
pub struct OperationContext {
    /// Unique identifier for this operation.
    pub request_id: Uuid,
    /// Optional total monotonic deadline for admission, transport, and execution.
    pub deadline: Option<Instant>,
    /// Optional human-readable caller intent for audit journals.
    pub intent: Option<String>,
}

impl Default for OperationContext {
    fn default() -> Self {
        return Self {
            request_id: Uuid::now_v7(),
            deadline: None,
            intent: None,
        };
    }
}

impl OperationContext {
    /// Create a fresh context with a time-ordered UUIDv7 request ID.
    #[must_use]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Attach a timeout duration from now as an absolute monotonic deadline.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Some(Instant::now() + timeout);

        return self;
    }

    /// Attach an explicit monotonic deadline.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);

        return self;
    }

    /// Attach human-readable intent for audit records.
    #[must_use]
    pub fn with_intent(mut self, intent: impl Into<String>) -> Self {
        self.intent = Some(intent.into());

        return self;
    }

    /// Attach an explicit request identifier.
    #[must_use]
    pub fn with_request_id(mut self, request_id: Uuid) -> Self {
        self.request_id = request_id;

        return self;
    }

    /// Compute the remaining duration until the deadline, or None if unbounded.
    #[must_use]
    pub fn remaining_duration(&self) -> Option<Duration> {
        let deadline = self.deadline?;
        let now = Instant::now();

        if now >= deadline {
            return Some(Duration::ZERO);
        }
        return Some(deadline - now);
    }

    /// Check whether the total deadline has expired.
    ///
    /// # Errors
    /// Returns `Error::ControlTimeout` if the deadline has passed.
    pub fn check_deadline(&self, phase: &'static str) -> Result<()> {
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(Error::ControlTimeout {
                session: PathBuf::from("<operation-context>"),
                phase,
                timeout_ms: 0,
                request_dispatched: false,
                source: IoError::new(IoErrorKind::TimedOut, "deadline expired"),
            });
        }
        return Ok(());
    }
}
