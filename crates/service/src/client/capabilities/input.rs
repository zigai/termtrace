use super::super::{
    client_error, ControlCommand, ControlPayload, MouseButton, PointerOperation, Result, Session,
};
use crate::actions::MutationGuard;
use serde::Serialize;
use termtrace_model::{
    execution::event::InputKind, terminal::observation::ActorBoundaries, value::ByteString,
};
use uuid::Uuid;

/// Mode-aware cell pointer and focus operations.
pub struct Pointer<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Pointer<'_> {
    /// Emit a press/release click plan at a zero-based cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn click(
        &self,
        column: u16,
        row: u16,
        button: MouseButton,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<ControlPayload> {
        return self
            .session
            .control(&guard.request(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Click { button },
                strict,
            }));
    }

    /// Emit signed vertical wheel ticks at a zero-based cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn scroll(
        &self,
        column: u16,
        row: u16,
        lines: i16,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<ControlPayload> {
        return self
            .session
            .control(&guard.request(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Scroll { lines },
                strict,
            }));
    }

    /// Report focus state when the application enabled focus reporting.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn focus(
        &self,
        focused: bool,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<ControlPayload> {
        return self
            .session
            .control(&guard.request(ControlCommand::Focus { focused, strict }));
    }
}

impl Pointer<'_> {
    /// Emit `count` complete press/release clicks at a zero-based cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn multi_click(
        &self,
        column: u16,
        row: u16,
        button: MouseButton,
        count: u8,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<PointerOutcome> {
        return self.typed(
            guard,
            ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::MultiClick { button, count },
                strict,
            },
        );
    }

    /// Press at a zero-based cell, emit `steps` motion samples, and release at the end cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn drag(
        &self,
        column: u16,
        row: u16,
        to_column: u16,
        to_row: u16,
        button: MouseButton,
        steps: u8,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<PointerOutcome> {
        return self.typed(
            guard,
            ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Drag {
                    button,
                    to_column,
                    to_row,
                    steps,
                },
                strict,
            },
        );
    }

    /// Resolve visible row-contained text and click its center from one actor-ordered state.
    ///
    /// # Errors
    /// Returns an error when the text is not visible, is ambiguous while `unique` is set, or the
    /// recorder cannot be reached or rejects the request; with `strict`, a click the application
    /// does not report is rejected instead of returning a semantic no-op.
    pub fn activate(
        &self,
        text: impl Into<String>,
        unique: bool,
        strict: bool,
        guard: &MutationGuard,
    ) -> Result<PointerOutcome> {
        return self.typed(
            guard,
            ControlCommand::Activate {
                text: text.into(),
                unique,
                strict,
            },
        );
    }

    fn typed(&self, guard: &MutationGuard, command: ControlCommand) -> Result<PointerOutcome> {
        let request = guard.request(command);
        let payload = self.session.control(&request)?;
        return PointerOutcome::from_payload(request.request_id, payload);
    }
}

/// Typed result of one pointer, focus, or text-activation request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointerOutcome {
    /// The actor encoded the request against live modes and wrote these exact bytes.
    Applied {
        /// Identity of the request, also its canonical operation identity.
        request_id: Uuid,
        /// Semantic input category.
        input_kind: InputKind,
        /// Exact bytes the actor wrote.
        bytes: ByteString,
        /// Number of bytes written completely.
        bytes_written: u64,
        /// Canonical input-intent event.
        intent_event_seq: u64,
        /// Canonical input-result event.
        result_event_seq: u64,
        /// Canonical positions after application.
        boundaries: ActorBoundaries,
    },
    /// The application does not report this input, so the actor correctly wrote nothing.
    Noop {
        /// Identity of the request.
        request_id: Uuid,
        /// Semantic input category.
        input_kind: InputKind,
        /// Stable explanation, such as `pointer_has_no_output` or `pointer_reporting_disabled`.
        reason: String,
        /// BLAKE3 of canonical JSON for the evaluated terminal modes.
        terminal_modes_hash: String,
        /// Canonical positions at evaluation.
        boundaries: ActorBoundaries,
    },
    /// Visible text was resolved and its center clicked.
    Activated {
        /// Identity of the request, also its canonical operation identity.
        request_id: Uuid,
        /// Exact matched text.
        text: String,
        /// Zero-based first matched column.
        start_column: u16,
        /// Zero-based exclusive end column.
        end_column: u16,
        /// Zero-based matched row.
        row: u16,
        /// Zero-based column that was clicked.
        target_column: u16,
        /// Presented-state hash the match was resolved in.
        presented_state_hash: String,
        /// Exact bytes the actor wrote.
        bytes: ByteString,
        /// Number of bytes written completely.
        bytes_written: u64,
        /// Canonical input-intent event.
        intent_event_seq: u64,
        /// Canonical input-result event.
        result_event_seq: u64,
        /// Canonical positions after application.
        boundaries: ActorBoundaries,
    },
}

impl PointerOutcome {
    /// Type the actor's answer to the pointer, focus, or activation request `request_id`.
    ///
    /// # Errors
    /// Returns an error when the payload is not a pointer, focus, or activation result.
    pub fn from_payload(request_id: Uuid, payload: ControlPayload) -> Result<Self> {
        return match payload {
            ControlPayload::InputApplied {
                input_kind,
                data,
                bytes_written,
                intent_event_seq,
                result_event_seq,
                boundaries,
                ..
            } => Ok(Self::Applied {
                request_id,
                input_kind,
                bytes: data,
                bytes_written,
                intent_event_seq,
                result_event_seq,
                boundaries,
            }),
            ControlPayload::InputNoop {
                input_kind,
                reason,
                terminal_modes_hash,
                boundaries,
            } => Ok(Self::Noop {
                request_id,
                input_kind,
                reason,
                terminal_modes_hash,
                boundaries,
            }),
            ControlPayload::Activated {
                text,
                start_column,
                end_column,
                row,
                target_column,
                presented_state_hash,
                data,
                bytes_written,
                intent_event_seq,
                result_event_seq,
                boundaries,
                ..
            } => Ok(Self::Activated {
                request_id,
                text,
                start_column,
                end_column,
                row,
                target_column,
                presented_state_hash,
                bytes: data,
                bytes_written,
                intent_event_seq,
                result_event_seq,
                boundaries,
            }),
            _ => Err(client_error("actor returned the wrong pointer payload")),
        };
    }
}
