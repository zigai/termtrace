//! Immutable actor-ordered action requests and observation results.

use std::time::Duration;
use termtrace_model::{
    protocol::control::{
        ActionInputResult, ScreenPostconditionQuality, ScreenPredicate, StableCaptureQuality,
    },
    terminal::observation::ActorBoundaries,
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::{KeyModifiers, NamedKey},
};
use uuid::Uuid;

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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ActionCondition {
    /// Return immediately after complete input application.
    #[default]
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
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
    /// Exact terminating screen frame, if returned.
    pub frame: Option<ScreenFrame>,
    /// Actor sequence boundaries enclosing the action.
    pub boundaries: ActorBoundaries,
}

/// Fluent builder for constructing atomic sequences of input atoms and action options.
#[derive(Clone, Debug, Default)]
pub struct ActionBuilder {
    atoms: Vec<InputAtom>,
    options: ActionOptions,
}

impl ActionBuilder {
    /// Construct a new empty action builder.
    #[must_use]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Append literal terminal text.
    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.atoms.push(InputAtom::Text(text.into()));

        return self;
    }

    /// Append raw terminal bytes.
    #[must_use]
    pub fn bytes(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.atoms.push(InputAtom::Bytes(bytes.into()));

        return self;
    }

    /// Append a symbolic named key with modifier flags.
    #[must_use]
    pub fn key(mut self, key: NamedKey, modifiers: KeyModifiers) -> Self {
        self.atoms.push(InputAtom::Key { key, modifiers });

        return self;
    }

    /// Append bracketed paste text.
    #[must_use]
    pub fn paste(mut self, text: impl Into<String>) -> Self {
        self.atoms.push(InputAtom::Paste(text.into()));

        return self;
    }

    /// Enforce an optimistic state guard before applying any mutation.
    #[must_use]
    pub fn if_state(mut self, state_hash: impl Into<String>) -> Self {
        self.options.if_state = Some(state_hash.into());

        return self;
    }

    /// Attach a human-readable caller intent.
    #[must_use]
    pub fn intent(mut self, intent: impl Into<String>) -> Self {
        self.options.caller_intent = Some(intent.into());

        return self;
    }

    /// Set pacing delay between atoms.
    #[must_use]
    pub fn pace(mut self, delay: Duration) -> Self {
        self.options.pace = delay;

        return self;
    }

    /// Await a screen condition after input application.
    #[must_use]
    pub fn until_screen(mut self, predicate: ScreenPredicate, timeout: Duration) -> Self {
        self.options.postcondition = ActionCondition::Screen { predicate, timeout };

        return self;
    }

    /// Await idle stability after input application.
    #[must_use]
    pub fn until_idle(mut self, stable_for: Duration, timeout: Duration) -> Self {
        self.options.postcondition = ActionCondition::Idle {
            stable_for,
            timeout,
        };

        return self;
    }

    /// Request a terminating frame projection.
    #[must_use]
    pub fn projection(mut self, projection: ScreenProjectionRequest) -> Self {
        self.options.projection = projection;

        return self;
    }

    /// Build into atoms and options tuple.
    #[must_use]
    pub fn build(self) -> (Vec<InputAtom>, ActionOptions) {
        return (self.atoms, self.options);
    }
}
