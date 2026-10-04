//! Ordered sends that mix single control commands with input actions.

use super::{client_error, Session};
use crate::{
    actions::{ActOutcome, ActRequest, ActionCondition, InputAtom, MutationGuard},
    screen::{StableCaptureOptions, StableScreenCapture},
    Error, Result,
};
use std::{error::Error as StdError, fmt, result::Result as StdResult, time::Duration};
use termtrace_model::{
    protocol::control::{ControlCommand, ControlPayload},
    terminal::{inspection::ScreenWaitOutcome, screen::ScreenProjectionRequest},
};
use uuid::Uuid;

/// One ordered step of [`Session::send`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendStep {
    /// One control command, such as a resize, signal, marker, or pointer operation.
    Command(ControlCommand),
    /// A run of input atoms applied as one actor-ordered action.
    Input(Vec<InputAtom>),
}

/// Ordered steps sent under one caller identity, with a condition checked after the last.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SendRequest {
    /// Steps applied in order; sending stops at the first failure.
    pub steps: Vec<SendStep>,
    /// Identity of the whole send and the presented-state guard for its first step.
    pub guard: MutationGuard,
    /// Optional human-readable intent retained with input operations.
    pub intent: Option<String>,
    /// Delay between consecutive input atoms.
    pub pace: Duration,
    /// Whether the actor should report outcomes for individual input atoms.
    pub verbose_atom_outcomes: bool,
    /// Condition checked after the last step; an unmet condition is reported, not an error.
    pub postcondition: ActionCondition,
    /// Detail and region requested for the terminating frame.
    pub projection: ScreenProjectionRequest,
}

/// What the last step of a [`Session::send`] produced.
#[derive(Debug)]
pub enum SendOutcome {
    /// The last step was input; its action outcome carries the postcondition.
    Action(Box<ActOutcome>),
    /// The last step was a control command, followed by the postcondition check.
    Command {
        /// Request identity of the command.
        request_id: Uuid,
        /// The actor's answer to the command.
        payload: Box<ControlPayload>,
        /// Postcondition checked after the command, unless none was requested.
        postcondition: Option<Box<CommandPostcondition>>,
    },
}

/// Postcondition checked after a send that ended with a control command.
#[derive(Debug)]
pub enum CommandPostcondition {
    /// Stable-capture result of an idle condition.
    Idle(Result<StableScreenCapture>),
    /// Screen-wait result of a screen condition.
    Screen(Result<ScreenWaitOutcome>),
}

/// The step that failed and stopped a [`Session::send`]; earlier steps were applied.
#[derive(Debug)]
pub struct SendFailure {
    /// Zero-based index of the failed step.
    pub step: usize,
    /// Request identity of the failed step, for retrying it.
    pub request_id: Uuid,
    /// Why the step failed.
    pub error: Box<Error>,
}

impl fmt::Display for SendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(f, "send step {} failed: {}", self.step, self.error);
    }
}

impl StdError for SendFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        return Some(self.error.as_ref());
    }
}

enum StepOutcome {
    Command(Box<ControlPayload>),
    Action(Box<ActOutcome>),
}

/// Request identity of step `index`: the send's own identity for the first step, and a stable
/// derivation for later ones, so retrying a whole send with its identity replays every step.
fn step_request_id(send_request_id: Uuid, index: usize) -> Uuid {
    if index == 0 {
        return send_request_id;
    }

    let name = format!("termtrace/send-step/v1/{send_request_id}/{index}");

    return Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes());
}

impl Session {
    /// Apply ordered steps, each a separate mutation with its own derived request identity.
    ///
    /// The presented-state guard applies to the first step only, because each step may change
    /// the presented state the next one starts from. The postcondition rides on the last step
    /// when it is input, and is checked after it when it is a control command.
    ///
    /// # Errors
    ///
    /// Returns the first failed step; steps before it were applied.
    pub fn send(&self, request: SendRequest) -> StdResult<SendOutcome, SendFailure> {
        let send_request_id = request.guard.request_id.unwrap_or_else(Uuid::now_v7);
        let Some((last_step, earlier_steps)) = request.steps.split_last() else {
            return Err(SendFailure {
                step: 0,
                request_id: send_request_id,
                error: Box::new(client_error("send requires at least one step")),
            });
        };

        for (index, step) in earlier_steps.iter().enumerate() {
            self.send_step(
                &request,
                send_request_id,
                index,
                step,
                ActionCondition::None,
            )?;
        }

        let last = earlier_steps.len();
        let request_id = step_request_id(send_request_id, last);

        return match self.send_step(
            &request,
            send_request_id,
            last,
            last_step,
            request.postcondition.clone(),
        )? {
            StepOutcome::Command(payload) => Ok(SendOutcome::Command {
                request_id,
                payload,
                postcondition: self.command_postcondition(&request),
            }),
            StepOutcome::Action(outcome) => Ok(SendOutcome::Action(outcome)),
        };
    }

    fn send_step(
        &self,
        request: &SendRequest,
        send_request_id: Uuid,
        index: usize,
        step: &SendStep,
        postcondition: ActionCondition,
    ) -> StdResult<StepOutcome, SendFailure> {
        let request_id = step_request_id(send_request_id, index);
        let guard = MutationGuard {
            request_id: Some(request_id),
            if_state: if index == 0 { request.guard.if_state.clone() } else { None },
        };

        let outcome = match step {
            SendStep::Command(command) => self
                .control(&guard.request(command.clone()))
                .map(|payload| return StepOutcome::Command(Box::new(payload))),
            SendStep::Input(atoms) => self
                .act(ActRequest {
                    atoms: atoms.clone(),
                    guard,
                    intent: request.intent.clone(),
                    pace: request.pace,
                    verbose_atom_outcomes: request.verbose_atom_outcomes,
                    postcondition,
                    projection: request.projection,
                    allow_incomplete: true,
                })
                .map(|outcome| return StepOutcome::Action(Box::new(outcome))),
        };

        return outcome.map_err(|error| {
            return SendFailure {
                step: index,
                request_id,
                error: Box::new(error),
            };
        });
    }

    fn command_postcondition(&self, request: &SendRequest) -> Option<Box<CommandPostcondition>> {
        let postcondition = match &request.postcondition {
            ActionCondition::None => return None,
            ActionCondition::Idle {
                stable_for,
                timeout,
            } => CommandPostcondition::Idle(self.capture_stable(StableCaptureOptions {
                stable_for: *stable_for,
                timeout: *timeout,
                projection: request.projection,
                allow_incomplete: true,
            })),
            ActionCondition::Screen { predicate, timeout } => CommandPostcondition::Screen(
                self.wait_screen(predicate.clone(), Duration::ZERO, *timeout),
            ),
        };

        return Some(Box::new(postcondition));
    }
}
