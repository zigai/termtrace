use super::screen::Captured;
use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use termtrace_model::protocol::control::{PointerOperation, ProcessSignal};
use termtrace_model::terminal::diff::ScreenChanges;
use termtrace_model::value::ExactPath;
use termtrace_sdk::client::Session;
use termtrace_sdk::contracts::control::{guard, parse_request_id, CommandOutcome, ErrorReport};
use termtrace_sdk::contracts::SendSpec;
use termtrace_sdk::input::{ActOutcome, MutationGuard, PointerOutcome};
use termtrace_sdk::screen::{ScreenWaitResult, StableScreenCapture};
use termtrace_sdk::session::{CommandPostcondition, SendLastStep, SendOutcome};
use termtrace_sdk::{ControlAcknowledgement, MarkOutcome, SessionStatus, Uuid};

/// The session a call addresses.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionRef {
    /// Session directory.
    path: ExactPath,
    /// Handle of the recording that owns the session, when this client started it; calls then
    /// fail once the recording stops or detaches.
    #[serde(default)]
    recording: Option<String>,
}

impl SessionRef {
    pub(crate) fn attach(&self, call: &Call<'_>) -> Result<Session, Failure> {
        return self.attach_as(call, Failure::sdk);
    }

    /// Attach, reporting an attach error through `failure`.
    pub(crate) fn attach_as(
        &self,
        call: &Call<'_>,
        failure: impl FnOnce(&termtrace_sdk::Error) -> Failure,
    ) -> Result<Session, Failure> {
        let path = self.path(call)?;

        return call
            .terminal()?
            .attach(path)
            .map_err(|error| return failure(&error));
    }

    /// The session directory, once an owning recording is known to be active.
    pub(crate) fn path(&self, call: &Call<'_>) -> Result<&Path, Failure> {
        if let Some(recording) = &self.recording {
            call.server.recordings.get(recording)?.ensure_active()?;
        }

        return Ok(&self.path.0);
    }
}

/// Identity and presented-state guard of one mutation.
#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mutation {
    /// Identity of this mutation, to retry it safely; allocated when absent.
    #[serde(default)]
    request_id: Option<String>,
    /// Apply only while the presented state has this hash.
    #[serde(default)]
    if_state: Option<String>,
}

impl Mutation {
    fn guard(&self) -> Result<MutationGuard, Failure> {
        let request_id = self
            .request_id
            .as_deref()
            .map(parse_request_id)
            .transpose()
            .map_err(|error| return Failure::sdk(&error))?;

        return Ok(guard(request_id, self.if_state.clone()));
    }
}

/// Attach and apply one mutation, attributing every failure to its request identity.
fn mutate<T>(
    call: &Call<'_>,
    session: &SessionRef,
    mutation: &Mutation,
    apply: impl FnOnce(&Session, &MutationGuard) -> termtrace_sdk::Result<T>,
) -> Result<T, Failure> {
    let guard = mutation.guard()?;
    let request_id = guard.request_id.unwrap_or_default();
    let session = session.attach_as(call, |error| return Failure::mutation(error, request_id))?;

    return apply(&session, &guard).map_err(|error| return Failure::mutation(&error, request_id));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusParams {
    session: SessionRef,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendParams {
    session: SessionRef,
    /// Ordered steps, guard, and postcondition.
    spec: SendSpec,
}

/// Outcome of an ordered send.
#[derive(Serialize, JsonSchema)]
struct SendResult {
    /// Identity of the whole send.
    request_id: Uuid,
    /// Outcome of the final step.
    last: SendLast,
    /// Screen changes across the send, when `report_changes` was set.
    changes: Option<ScreenChanges>,
}

/// Outcome of the final step of an ordered send.
#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SendLast {
    /// The final step was input.
    Action {
        /// Zero-based step index.
        step: usize,
        /// Identity of the input action.
        request_id: Uuid,
        /// Complete action result.
        result: Box<ActOutcome>,
    },
    /// The final step was a control command.
    Command {
        /// Zero-based step index.
        step: usize,
        /// Identity of the command.
        request_id: Uuid,
        /// Command outcome.
        outcome: CommandOutcome,
        /// Postcondition checked after the command, when one was requested.
        postcondition: Option<Box<CommandCheck>>,
    },
}

/// Postcondition checked after a final control command.
#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CommandCheck {
    /// Stable-screen capture.
    Idle {
        /// The capture, when it succeeded.
        capture: Option<Box<Captured<StableScreenCapture>>>,
        /// Why the capture failed.
        error: Option<ErrorReport>,
    },
    /// Screen predicate wait.
    Screen {
        /// The wait result, when it ran.
        wait: Option<Box<Captured<ScreenWaitResult>>>,
        /// Why the wait failed.
        error: Option<ErrorReport>,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResizeParams {
    session: SessionRef,
    /// Columns.
    cols: u16,
    /// Rows.
    rows: u16,
    #[serde(default)]
    mutation: Mutation,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SignalParams {
    session: SessionRef,
    /// Signal for the child process group.
    signal: ProcessSignal,
    #[serde(default)]
    mutation: Mutation,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkParams {
    session: SessionRef,
    /// Nonempty marker label.
    label: String,
    #[serde(default)]
    mutation: Mutation,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PointerParams {
    session: SessionRef,
    /// Zero-based cell column.
    column: u16,
    /// Zero-based cell row.
    row: u16,
    /// Click, multi-click, drag, or scroll.
    operation: PointerOperation,
    /// Reject instead of a no-op when the application does not report mouse input.
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    mutation: Mutation,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FocusParams {
    session: SessionRef,
    /// Whether the terminal gains or loses focus.
    focused: bool,
    /// Reject instead of a no-op when the application does not report focus.
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    mutation: Mutation,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivateParams {
    session: SessionRef,
    /// Visible text to click.
    text: String,
    /// Require exactly one match.
    #[serde(default)]
    unique: bool,
    /// Reject instead of a no-op when the application does not report mouse input.
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    mutation: Mutation,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "session.status",
        "Live status, or the completion of a finished session.",
        status,
    );
    method(
        registry,
        "session.send",
        "Apply ordered input and control steps, then check an optional postcondition.",
        send,
    );
    method(registry, "session.resize", "Resize the terminal.", resize);
    method(
        registry,
        "session.signal",
        "Send a signal to the child process group.",
        signal,
    );
    method(
        registry,
        "session.mark",
        "Append a named marker to the event stream.",
        mark,
    );
    method(
        registry,
        "session.pointer",
        "Click, multi-click, drag, or scroll at a cell.",
        pointer,
    );
    method(
        registry,
        "session.focus",
        "Report a terminal focus change to the application.",
        focus,
    );
    method(
        registry,
        "session.activate",
        "Click the visible text.",
        activate,
    );
}

fn status(call: &Call<'_>, params: StatusParams) -> Result<SessionStatus, Failure> {
    return params
        .session
        .attach(call)?
        .status()
        .map_err(|error| return Failure::sdk(&error));
}

fn send(call: &Call<'_>, params: SendParams) -> Result<SendResult, Failure> {
    let request = params
        .spec
        .to_request()
        .map_err(|error| return Failure::sdk(&error))?;

    let send_request_id = request.guard.request_id.unwrap_or_default();
    let last_step = request.steps.len().saturating_sub(1);
    let session = params.session.attach_as(call, |error| {
        return Failure::mutation(error, send_request_id);
    })?;
    let outcome = session
        .send(request)
        .map_err(|failure| return Failure::send(failure, send_request_id))?;

    return send_result(send_request_id, last_step, outcome);
}

fn send_result(
    send_request_id: Uuid,
    step: usize,
    outcome: SendOutcome,
) -> Result<SendResult, Failure> {
    let last = match outcome.last {
        SendLastStep::Action(action) => SendLast::Action {
            step,
            request_id: action.request_id,
            result: action,
        },
        SendLastStep::Command {
            request_id,
            payload,
            postcondition,
        } => {
            let command = CommandOutcome::from_payload(request_id, &payload)
                .map_err(|error| return Failure::sdk(&error))?;

            let postcondition = postcondition.map(|postcondition| {
                return Box::new(match *postcondition {
                    CommandPostcondition::Idle(Ok(capture)) => CommandCheck::Idle {
                        capture: Some(Box::new(Captured::new(capture.clone(), capture.frame))),
                        error: None,
                    },
                    CommandPostcondition::Idle(Err(error)) => CommandCheck::Idle {
                        capture: None,
                        error: Some(ErrorReport::from(&error)),
                    },
                    CommandPostcondition::Screen(Ok(wait)) => CommandCheck::Screen {
                        wait: Some(Box::new(Captured::new(
                            ScreenWaitResult::from(&wait),
                            wait.frame,
                        ))),
                        error: None,
                    },
                    CommandPostcondition::Screen(Err(error)) => CommandCheck::Screen {
                        wait: None,
                        error: Some(ErrorReport::from(&error)),
                    },
                });
            });

            SendLast::Command {
                step,
                request_id,
                outcome: command,
                postcondition,
            }
        }
    };

    return Ok(SendResult {
        request_id: send_request_id,
        last,
        changes: outcome.changes,
    });
}

fn resize(call: &Call<'_>, params: ResizeParams) -> Result<ControlAcknowledgement, Failure> {
    return mutate(call, &params.session, &params.mutation, |session, guard| {
        return session.resize(params.cols, params.rows, guard);
    });
}

fn signal(call: &Call<'_>, params: SignalParams) -> Result<ControlAcknowledgement, Failure> {
    return mutate(call, &params.session, &params.mutation, |session, guard| {
        return session.signal(params.signal, guard);
    });
}

fn mark(call: &Call<'_>, params: MarkParams) -> Result<MarkOutcome, Failure> {
    return mutate(call, &params.session, &params.mutation, |session, guard| {
        return session.mark(params.label, guard);
    });
}

fn pointer(call: &Call<'_>, params: PointerParams) -> Result<PointerOutcome, Failure> {
    let PointerParams {
        session,
        column,
        row,
        operation,
        strict,
        mutation,
    } = params;

    return mutate(call, &session, &mutation, |session, guard| {
        let request_id = guard.request_id.unwrap_or_default();
        let pointer = session.pointer();

        return match operation {
            PointerOperation::Click { button } => pointer
                .click(column, row, button, strict, guard)
                .and_then(|payload| return PointerOutcome::from_payload(request_id, payload)),
            PointerOperation::Scroll { lines } => pointer
                .scroll(column, row, lines, strict, guard)
                .and_then(|payload| return PointerOutcome::from_payload(request_id, payload)),
            PointerOperation::MultiClick { button, count } => {
                pointer.multi_click(column, row, button, count, strict, guard)
            }
            PointerOperation::Drag {
                button,
                to_column,
                to_row,
                steps,
            } => pointer.drag(column, row, to_column, to_row, button, steps, strict, guard),
        };
    });
}

fn focus(call: &Call<'_>, params: FocusParams) -> Result<PointerOutcome, Failure> {
    return mutate(call, &params.session, &params.mutation, |session, guard| {
        let request_id = guard.request_id.unwrap_or_default();

        return session
            .pointer()
            .focus(params.focused, params.strict, guard)
            .and_then(|payload| return PointerOutcome::from_payload(request_id, payload));
    });
}

fn activate(call: &Call<'_>, params: ActivateParams) -> Result<PointerOutcome, Failure> {
    return mutate(call, &params.session, &params.mutation, |session, guard| {
        return session
            .pointer()
            .activate(params.text, params.unique, params.strict, guard);
    });
}
