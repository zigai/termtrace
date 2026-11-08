//! Immutable actor-ordered action requests and their outcomes.

use crate::defaults;
use std::time::Duration;
use termtrace_model::{
    execution::event::InputKind,
    protocol::control::{
        derive_input_operation_id, ActionInputResult, ActionPostcondition,
        ActionPostconditionResult, ControlCommand, ControlRequest, InputSequenceAtom,
        ReactionRequirement, ScreenPredicate,
    },
    terminal::observation::ActorBoundaries,
    terminal::screen::{FrameDetail, ScreenFrame, ScreenProjectionRequest},
    terminal::state::KeyChord,
    value::ByteString,
};
use uuid::Uuid;

/// One immutable user-intent atom applied by `Session::act`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputAtom {
    /// Pause before the next atom without splitting textual content.
    Sleep(Duration),
    /// UTF-8 terminal text; typed one scalar per atom when the action is paced.
    Text(String),
    /// Exact raw terminal bytes.
    Bytes(Vec<u8>),
    /// One key chord encoded against live terminal modes.
    Key(KeyChord),
    /// UTF-8 paste encoded against live bracketed-paste mode.
    Paste(String),
}

/// What must hold after input is applied, evaluated under the same actor ordering point.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ActionCondition {
    /// Return immediately after complete input application.
    #[default]
    None,
    /// Require a reaction, then a presented state that stays unchanged.
    Idle {
        /// Required interval during which the presented state remains unchanged.
        stable_for: Duration,
        /// Maximum time to wait for stability.
        timeout: Duration,
        /// Reaction that must precede the stable interval.
        reaction: Reaction,
    },
    /// Evaluate one actor-portable presented-screen predicate.
    Screen {
        /// Presented-screen condition evaluated by the actor.
        predicate: ScreenPredicate,
        /// Maximum time to wait for a matching screen.
        timeout: Duration,
        /// Required unchanged interval after a match.
        settle: Duration,
    },
}

impl ActionCondition {
    /// Wait for the default idle interval within the default wait timeout.
    #[must_use]
    pub const fn idle() -> Self {
        return Self::Idle {
            stable_for: defaults::IDLE_STABLE_FOR,
            timeout: defaults::WAIT_TIMEOUT,
            reaction: Reaction::new(),
        };
    }

    /// Wire form of this condition.
    #[must_use]
    pub fn to_wire(&self) -> ActionPostcondition {
        return match self {
            Self::None => ActionPostcondition::None,
            Self::Idle {
                stable_for,
                timeout,
                reaction,
            } => ActionPostcondition::Idle {
                stable_for_ns: duration_ns(*stable_for),
                timeout_ns: duration_ns(*timeout),
                reaction: reaction.to_wire(),
            },
            Self::Screen {
                predicate,
                timeout,
                settle,
            } => ActionPostcondition::Screen {
                predicate: predicate.clone(),
                timeout_ns: duration_ns(*timeout),
                stable_for_ns: duration_ns(*settle),
            },
        };
    }
}

/// Output or a presented-state change required before stability is measured.
///
/// Without one within `timeout`, or the stable interval when longer, the condition settles
/// as `unchanged`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reaction {
    /// Presented-state hash the screen must leave; `None` uses the state when the actor
    /// admitted the request.
    pub changed_from: Option<String>,
    /// How long to wait for a reaction.
    pub timeout: Duration,
}

impl Reaction {
    /// React relative to the admitted state within the default reaction timeout.
    #[must_use]
    pub const fn new() -> Self {
        return Self {
            changed_from: None,
            timeout: defaults::REACTION_TIMEOUT,
        };
    }

    /// Wire form of this reaction.
    #[must_use]
    pub fn to_wire(&self) -> ReactionRequirement {
        return ReactionRequirement {
            changed_from: self.changed_from.clone(),
            timeout_ns: duration_ns(self.timeout),
        };
    }
}

impl Default for Reaction {
    fn default() -> Self {
        return Self::new();
    }
}

/// Caller-owned identity and optimistic presented-state guard for one mutation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MutationGuard {
    /// Caller-retained request identity for safe retries, or `None` for a fresh UUID.
    pub request_id: Option<Uuid>,
    /// Presented-state hash that must still be current before mutation.
    pub if_state: Option<String>,
}

impl MutationGuard {
    /// Wire request for `command` carrying this guard's identity and presented-state guard.
    #[must_use]
    pub fn request(&self, command: ControlCommand) -> ControlRequest {
        let mut request = ControlRequest::new(command);
        if let Some(request_id) = self.request_id {
            request.request_id = request_id;
        }

        request
            .expected_presented_state_hash
            .clone_from(&self.if_state);

        return request;
    }
}

/// One actor-ordered input action and the condition that must hold afterward.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActRequest {
    /// Ordered input atoms.
    pub atoms: Vec<InputAtom>,
    /// Request identity and presented-state guard.
    pub guard: MutationGuard,
    /// Optional human-readable intent retained with the input operations.
    pub intent: Option<String>,
    /// Delay between consecutive atoms.
    pub pace: Duration,
    /// Whether the actor should report outcomes for individual atoms.
    pub verbose_atom_outcomes: bool,
    /// Completion condition evaluated after all input is applied.
    pub postcondition: ActionCondition,
    /// Detail and region requested for the terminating frame.
    pub projection: ScreenProjectionRequest,
    /// Whether an unmet postcondition is returned as an outcome instead of an error.
    pub allow_incomplete: bool,
}

impl ActRequest {
    /// Start an empty action.
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

    /// Append one key chord.
    #[must_use]
    pub fn key(mut self, chord: KeyChord) -> Self {
        self.atoms.push(InputAtom::Key(chord));

        return self;
    }

    /// Append bracketed paste text.
    #[must_use]
    pub fn paste(mut self, text: impl Into<String>) -> Self {
        self.atoms.push(InputAtom::Paste(text.into()));

        return self;
    }

    /// Apply only while the presented state still has this hash.
    #[must_use]
    pub fn if_state(mut self, state_hash: impl Into<String>) -> Self {
        self.guard.if_state = Some(state_hash.into());

        return self;
    }

    /// Attach a human-readable caller intent.
    #[must_use]
    pub fn intent(mut self, intent: impl Into<String>) -> Self {
        self.intent = Some(intent.into());

        return self;
    }

    /// Delay between consecutive atoms.
    #[must_use]
    pub const fn pace(mut self, delay: Duration) -> Self {
        self.pace = delay;

        return self;
    }

    /// Await a screen predicate after input application.
    #[must_use]
    pub fn until_screen(mut self, predicate: ScreenPredicate, timeout: Duration) -> Self {
        self.postcondition = ActionCondition::Screen {
            predicate,
            timeout,
            settle: Duration::ZERO,
        };

        return self;
    }

    /// Await a reaction to the input and then an unchanged presented state.
    #[must_use]
    pub fn until_idle(mut self, stable_for: Duration, timeout: Duration) -> Self {
        self.postcondition = ActionCondition::Idle {
            stable_for,
            timeout,
            reaction: Reaction::new(),
        };

        return self;
    }

    /// Request a terminating frame projection.
    #[must_use]
    pub const fn projection(mut self, projection: ScreenProjectionRequest) -> Self {
        self.projection = projection;

        return self;
    }

    /// Wire atoms for this action: paced text is typed one Unicode scalar per atom, and
    /// each atom's operation identity derives from the request identity and its position.
    #[must_use]
    pub fn wire_atoms(&self, request_id: Uuid) -> Vec<InputSequenceAtom> {
        let mut wire = Vec::with_capacity(self.atoms.len());
        for atom in &self.atoms {
            match atom {
                InputAtom::Text(text) if !self.pace.is_zero() => {
                    for scalar in text.chars() {
                        let operation_id = derive_input_operation_id(request_id, wire.len());
                        wire.push(InputSequenceAtom::Bytes {
                            operation_id,
                            data: ByteString::new(scalar.to_string().into_bytes()),
                            input_kind: InputKind::Text,
                            caller_intent: self.intent.clone(),
                        });
                    }
                }
                atom => {
                    let operation_id = derive_input_operation_id(request_id, wire.len());
                    wire.push(wire_atom(atom, operation_id, self.intent.clone()));
                }
            }
        }

        return wire;
    }
}

fn wire_atom(
    atom: &InputAtom,
    operation_id: Uuid,
    caller_intent: Option<String>,
) -> InputSequenceAtom {
    return match atom {
        InputAtom::Sleep(duration) => InputSequenceAtom::Sleep {
            operation_id,
            duration_ns: duration_ns(*duration),
        },
        InputAtom::Text(text) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            input_kind: InputKind::Text,
            caller_intent,
        },
        InputAtom::Bytes(bytes) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(bytes.clone()),
            input_kind: InputKind::Raw,
            caller_intent,
        },
        InputAtom::Key(chord) => InputSequenceAtom::Key {
            operation_id,
            key: chord.key,
            modifiers: chord.modifiers,
            caller_intent,
        },
        InputAtom::Paste(text) => InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            caller_intent,
        },
    };
}

/// Complete result of one actor-ordered action.
#[derive(
    Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ActOutcome {
    /// Identity of the action request.
    pub request_id: Uuid,
    /// Ordered input application outcome.
    pub input: ActionInputResult,
    /// Postcondition result exactly as the actor reported it.
    pub postcondition: ActionPostconditionResult,
    /// Actor sequence boundaries enclosing the action.
    pub boundaries: ActorBoundaries,
}

impl ActOutcome {
    /// Whether the requested postcondition held.
    #[must_use]
    pub const fn is_matched(&self) -> bool {
        return match &self.postcondition {
            ActionPostconditionResult::None { matched, .. }
            | ActionPostconditionResult::Idle { matched, .. }
            | ActionPostconditionResult::Screen { matched, .. } => *matched,
        };
    }

    /// Exact terminating frame, when one was projected.
    #[must_use]
    pub const fn frame(&self) -> Option<&ScreenFrame> {
        return match &self.postcondition {
            ActionPostconditionResult::None { frame, .. }
            | ActionPostconditionResult::Idle { frame, .. }
            | ActionPostconditionResult::Screen { frame, .. } => frame.as_ref(),
        };
    }

    /// Terminating presented-state hash, from the actor or its frame.
    #[must_use]
    pub fn state_hash(&self) -> Option<&str> {
        let (ActionPostconditionResult::None { state_hash, .. }
        | ActionPostconditionResult::Idle { state_hash, .. }
        | ActionPostconditionResult::Screen { state_hash, .. }) = &self.postcondition;

        return state_hash
            .as_deref()
            .or_else(|| return self.frame().map(|frame| return frame.state_hash.as_str()));
    }
}

/// Projection that returns only the terminating state hash, without frame content.
#[must_use]
pub const fn hash_only_projection() -> ScreenProjectionRequest {
    return ScreenProjectionRequest {
        detail: FrameDetail::None,
        region: None,
    };
}

/// Nanoseconds in a duration, saturating at the protocol's unsigned 64-bit range.
#[must_use]
pub fn duration_ns(duration: Duration) -> u64 {
    return u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
}
