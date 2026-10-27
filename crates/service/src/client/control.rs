//! Typed session-control mutations: resize, signal, and canonical markers.

use super::{client_error, Session};
use crate::{actions::MutationGuard, defaults, Result};
use serde::Serialize;
use termtrace_model::{
    protocol::control::{ControlCommand, ControlPayload, ProcessSignal},
    terminal::observation::ActorBoundaries,
};
use uuid::Uuid;

/// The actor applied a control mutation that produces no further data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ControlAcknowledgement {
    /// Identity of the request.
    pub request_id: Uuid,
    /// Canonical positions after the mutation.
    pub boundaries: ActorBoundaries,
}

/// Canonical position of an appended marker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct MarkOutcome {
    /// Identity of the request.
    pub request_id: Uuid,
    /// Normalized marker label.
    pub label: String,
    /// Canonical event containing the marker.
    pub event_seq: u64,
    /// Canonical positions after the marker.
    pub boundaries: ActorBoundaries,
}

impl Session {
    /// Change the child PTY to `cols` by `rows` cells, with pixel dimensions derived from the
    /// default cell size.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection, such as an unsupported geometry, or a transport failure.
    pub fn resize(
        &self,
        cols: u16,
        rows: u16,
        guard: &MutationGuard,
    ) -> Result<ControlAcknowledgement> {
        return self.acknowledged(
            guard,
            ControlCommand::Resize {
                cols,
                rows,
                pixel_width: cols.saturating_mul(defaults::CELL_WIDTH),
                pixel_height: rows.saturating_mul(defaults::CELL_HEIGHT),
            },
        );
    }

    /// Send one allow-listed signal to the child process group.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection or a transport failure.
    pub fn signal(
        &self,
        signal: ProcessSignal,
        guard: &MutationGuard,
    ) -> Result<ControlAcknowledgement> {
        return self.acknowledged(guard, ControlCommand::Signal { signal });
    }

    /// Append a named marker to the canonical event stream.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty label, the actor's rejection, or a transport failure.
    pub fn mark(&self, label: impl Into<String>, guard: &MutationGuard) -> Result<MarkOutcome> {
        let request = guard.request(ControlCommand::Mark {
            label: label.into(),
        });

        let ControlPayload::Marked {
            label,
            event_seq,
            boundaries,
        } = self.control(&request)?
        else {
            return Err(client_error("actor returned the wrong marker payload"));
        };

        return Ok(MarkOutcome {
            request_id: request.request_id,
            label,
            event_seq,
            boundaries,
        });
    }

    fn acknowledged(
        &self,
        guard: &MutationGuard,
        command: ControlCommand,
    ) -> Result<ControlAcknowledgement> {
        let request = guard.request(command);
        let ControlPayload::Acknowledged { boundaries } = self.control(&request)? else {
            return Err(client_error(
                "actor returned the wrong acknowledgement payload",
            ));
        };

        return Ok(ControlAcknowledgement {
            request_id: request.request_id,
            boundaries,
        });
    }
}
