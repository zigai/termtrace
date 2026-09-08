use nix::poll::{PollFd, PollFlags};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    protocol::control::{ControlResponse, ControlResult, CONTROL_PROTOCOL_VERSION},
    terminal::observation::ActorBoundaries,
};
use uuid::Uuid;

use crate::{Error, Result};

pub(in crate::recording) fn poll_ready(descriptor: &PollFd<'_>) -> bool {
    return descriptor
        .revents()
        .unwrap_or(PollFlags::empty())
        .intersects(
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        );
}

pub(in crate::recording) fn actor_boundaries(event_log: &EventLogWriter) -> ActorBoundaries {
    return ActorBoundaries {
        durable: event_log.durable_boundary(),
        observed: event_log.observed_boundary(),
    };
}

pub(in crate::recording) fn validate_control_input(
    data: &[u8],
    intent: Option<&str>,
) -> Result<()> {
    if data.is_empty() || data.len() > 64 * 1024 {
        return Err(control_rejected(
            "invalid_input",
            "input must contain 1 to 65536 bytes",
        ));
    }

    if intent.is_some_and(|value| return value.len() > 1024) {
        return Err(control_rejected(
            "invalid_intent",
            "input intent must not exceed 1024 UTF-8 bytes",
        ));
    }

    return Ok(());
}

pub(in crate::recording) fn control_rejected(code: &str, message: &str) -> Error {
    return Error::ControlRejected {
        code: code.to_owned(),
        message: message.to_owned(),
    };
}

pub(in crate::recording) fn control_error_response(
    request_id: Uuid,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ControlResponse {
    return ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: code.into(),
            message: message.into(),
            details: None,
        },
    };
}

pub(in crate::recording) fn finish_with_cleanup<T>(
    primary: Result<T>,
    cleanup: Result<()>,
) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Ok(())) => return Err(primary),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

pub(in crate::recording) fn terminal_error(
    operation: &'static str,
    reason: impl Into<String>,
) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}
