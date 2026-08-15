//! Immutable actor-ordered action requests and observation results.

use std::time::Duration;

use uuid::Uuid;

use termtrace_model::{
    protocol::control::{
        ActionInputResult, ScreenPostconditionQuality, ScreenPredicate, StableCaptureQuality,
    },
    terminal::observation::ActorBoundaries,
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::{KeyModifiers, NamedKey},
};

/// One immutable user-intent atom accepted by `Session::act`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputAtom {
    /// UTF-8 terminal text.
    Text(String),
    /// Exact raw terminal bytes.
    Bytes(Vec<u8>),
    /// One symbolic key encoded against live terminal modes.
    Key {
        /// Named key to encode.
        key: NamedKey,
        /// Modifiers held while applying the key.
        modifiers: KeyModifiers,
    },
    /// UTF-8 paste encoded against live bracketed-paste mode.
    Paste(String),
}

/// Typed public postcondition using exact Rust durations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionCondition {
    /// Return immediately after complete input application.
    None,
    /// Require one unchanged presented state.
    Idle {
        /// Required interval during which the presented state remains unchanged.
        stable_for: Duration,
        /// Maximum time to wait for stability.
        timeout: Duration,
    },
    /// Evaluate one actor-portable presented-screen predicate.
    Screen {
        /// Presented-screen condition evaluated by the actor.
        predicate: ScreenPredicate,
        /// Maximum time to wait for a matching screen.
        timeout: Duration,
    },
}

/// Public options for one actor-ordered input action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionOptions {
    /// Caller-supplied request identity, or `None` to allocate a fresh UUID.
    pub request_id: Option<Uuid>,
    /// Presented-state hash that must still be current before mutation.
    pub if_state: Option<String>,
    /// Optional human-readable intent retained with the input operations.
    pub caller_intent: Option<String>,
    /// Delay between consecutive input atoms.
    pub pace: Duration,
    /// Whether the actor should retain outcomes for individual atoms.
    pub verbose_atom_outcomes: bool,
    /// Completion condition evaluated after all input is applied.
    pub postcondition: ActionCondition,
    /// Detail and region requested for the terminating frame.
    pub projection: ScreenProjectionRequest,
    /// Whether to return an incomplete postcondition result instead of an error.
    pub allow_incomplete: bool,
}

impl Default for ActionOptions {
    fn default() -> Self {
        return Self {
            request_id: None,
            if_state: None,
            caller_intent: None,
            pace: Duration::ZERO,
            verbose_atom_outcomes: false,
            postcondition: ActionCondition::None,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        };
    }
}

/// Typed public action completion without a protocol envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionCompletion {
    /// No postcondition was requested.
    None,
    /// Outcome of waiting for an unchanged presented state.
    Idle {
        /// Whether the requested unchanged interval was reached.
        matched: bool,
        /// Stability evidence reported by the actor.
        quality: StableCaptureQuality,
    },
    /// Outcome of waiting for a presented-screen predicate.
    Screen {
        /// Whether the screen predicate matched before its deadline.
        matched: bool,
        /// Screen-predicate evidence reported by the actor.
        quality: ScreenPostconditionQuality,
    },
}

/// Complete owned result of one actor-ordered action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionResult {
    /// Identity of the action request.
    pub request_id: Uuid,
    /// Ordered input application outcome.
    pub input: ActionInputResult,
    /// Postcondition outcome.
    pub completion: ActionCompletion,
    /// Exact terminating screen frame.
    pub frame: ScreenFrame,
    /// Actor sequence boundaries enclosing the action.
    pub boundaries: ActorBoundaries,
}
