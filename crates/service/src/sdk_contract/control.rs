//! Interaction and session-control contracts shared by the Python and Node bindings: builder
//! sealing, host option shapes, and host result shapes.
//!
//! Host option shapes are model-style values with millisecond durations.

use crate::{
    actions::{ActRequest, ActionCondition, InputAtom, MutationGuard, Reaction},
    client::{
        ControlAcknowledgement, LaunchSpec, MarkOutcome, OriginalFps, OriginalGeometry,
        OriginalLaunchSpec, PointerOutcome, SendRequest, SendStep, StopOutcome,
    },
    defaults, Error, Result,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, mem, path::PathBuf, time::Duration};
use termtrace_model::{
    artifact::original::{CellGrid, PixelCanvas},
    execution::environment::EnvironmentSpec,
    execution::visual_profile::VisualProfile,
    protocol::control::{
        ActionPostconditionResult, ControlCommand, ControlPayload, PointerOperation, ProcessSignal,
        ScreenPostconditionQuality, ScreenPredicate, StableCaptureQuality, MAX_CONTROL_WAIT,
    },
    protocol::mutation::{MutationEffect, Retry},
    terminal::{
        screen::{FrameDetail, ScreenProjectionRequest, ScreenRegion},
        state::KeyChord,
    },
    value::ByteString,
};
use uuid::Uuid;

fn invalid(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
        source: None,
    };
}

/// Parse a caller-owned request identity: a hyphenated, non-nil UUID.
///
/// # Errors
///
/// Returns `invalid_operation` for any other text.
pub fn parse_request_id(text: &str) -> Result<Uuid> {
    let parsed = Uuid::try_parse(text)
        .ok()
        .filter(|id| return !id.is_nil() && id.hyphenated().to_string().eq_ignore_ascii_case(text));

    return parsed.ok_or_else(|| {
        return invalid(format!(
            "request identity `{text}` is not a hyphenated non-nil UUID"
        ));
    });
}

/// Mutation guard with a caller-owned identity, or a fresh one when none is given.
#[must_use]
pub fn guard(request_id: Option<Uuid>, if_state: Option<String>) -> MutationGuard {
    return MutationGuard {
        request_id: Some(request_id.unwrap_or_else(Uuid::now_v7)),
        if_state,
    };
}

/// Builder state shared by the bindings: an editable action whose identity exists from
/// construction and that becomes immutable once sealed.
///
/// Sealing freezes the exact request so every later send is a same-identity retry. The legacy
/// fresh-request path sends the current request and starts over with a new identity.
#[derive(Clone, Debug)]
pub struct ActionDraft {
    request: ActRequest,
    sealed: bool,
}

impl Default for ActionDraft {
    fn default() -> Self {
        return Self::new();
    }
}

impl ActionDraft {
    /// Start an empty action with a freshly allocated identity.
    #[must_use]
    pub fn new() -> Self {
        let mut request = ActRequest::new();
        request.guard.request_id = Some(Uuid::now_v7());

        return Self {
            request,
            sealed: false,
        };
    }

    /// Identity the next send uses.
    #[must_use]
    pub fn request_id(&self) -> Uuid {
        return self.request.guard.request_id.unwrap_or_default();
    }

    /// Whether the request is frozen.
    #[must_use]
    pub const fn is_sealed(&self) -> bool {
        return self.sealed;
    }

    fn unsealed(&self, change: &str) -> Result<()> {
        if self.sealed {
            return Err(invalid(format!(
                "cannot {change} a sealed action; run() retries the frozen request {}",
                self.request_id()
            )));
        }
        return Ok(());
    }

    /// Change the unsealed request.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation`, leaving the request unchanged, once sealed.
    pub fn edit(&mut self, change: impl FnOnce(&mut ActRequest)) -> Result<()> {
        self.unsealed("change")?;
        change(&mut self.request);
        return Ok(());
    }

    /// Replace the pre-allocated identity before sealing.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation`, leaving the identity unchanged, once sealed.
    pub fn set_request_id(&mut self, request_id: Uuid) -> Result<()> {
        self.unsealed("change the request identity of")?;
        self.request.guard.request_id = Some(request_id);
        return Ok(());
    }

    /// Freeze the request, idempotently, and return it.
    pub fn seal(&mut self) -> ActRequest {
        self.sealed = true;

        return self.request.clone();
    }

    /// Take the current request as a fresh legacy send and start over empty with a new identity.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation`, leaving the request unchanged, once sealed.
    pub fn take_fresh(&mut self) -> Result<ActRequest> {
        self.unsealed("execute")?;

        return Ok(mem::replace(&mut self.request, Self::new().request));
    }
}

fn millis(milliseconds: u64, what: &str) -> Result<Duration> {
    let duration = Duration::from_millis(milliseconds);
    if duration > MAX_CONTROL_WAIT {
        return Err(invalid(format!(
            "{what} of {milliseconds} ms exceeds the maximum of {} ms",
            MAX_CONTROL_WAIT.as_millis()
        )));
    }
    return Ok(duration);
}

fn optional_millis(milliseconds: Option<u64>, default: Duration, what: &str) -> Result<Duration> {
    return milliseconds.map_or(Ok(default), |milliseconds| {
        return millis(milliseconds, what);
    });
}

/// Parse one key chord with the shared grammar, e.g. `Enter`, `ctrl+c`, `ctrl+shift+F5`.
///
/// # Errors
///
/// Returns `invalid_operation` when the chord is malformed or names an unknown key.
pub fn parse_key(key: &str) -> Result<KeyChord> {
    return KeyChord::parse(key)
        .map_err(|error| return invalid(error.to_string()))?
        .ok_or_else(|| {
            return invalid(format!(
                "unknown key name: '{key}'; send plain characters as text"
            ));
        });
}

/// One input atom in host form: `{"text"}`, `{"bytes"}`, `{"key"}`, or `{"paste"}`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AtomSpec {
    /// UTF-8 text.
    Text(String),
    /// Exact raw bytes.
    Bytes(ByteString),
    /// One key chord in the shared grammar.
    Key(String),
    /// Bracketed-paste text.
    Paste(String),
}

impl AtomSpec {
    /// The input atom this describes.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for an invalid key chord.
    pub fn to_atom(&self) -> Result<InputAtom> {
        return Ok(match self {
            Self::Text(text) => InputAtom::Text(text.clone()),
            Self::Bytes(bytes) => InputAtom::Bytes(bytes.as_bytes().to_vec()),
            Self::Key(key) => InputAtom::Key(parse_key(key)?),
            Self::Paste(text) => InputAtom::Paste(text.clone()),
        });
    }
}

/// Reaction required before an idle interval, in host millisecond units.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReactionSpec {
    /// Presented-state hash the screen must leave; absent means the state at admission.
    #[serde(default)]
    pub changed_from: Option<String>,
    /// How long to wait for a reaction.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Condition checked after input, in host millisecond units.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "condition", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConditionSpec {
    /// Return once input is applied.
    None,
    /// Require a reaction, then an unchanged presented state.
    Idle {
        /// Required unchanged interval.
        #[serde(default)]
        stable_for_ms: Option<u64>,
        /// Maximum wait.
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// Reaction preceding the interval.
        #[serde(default)]
        reaction: Option<ReactionSpec>,
    },
    /// Wait for one presented-screen predicate.
    Screen {
        /// Presented-screen predicate.
        predicate: ScreenPredicate,
        /// Maximum wait.
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

impl ConditionSpec {
    /// The action condition this describes; absent durations take the shared defaults.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for a duration above the control wait bound.
    pub fn to_condition(&self) -> Result<ActionCondition> {
        return Ok(match self {
            Self::None => ActionCondition::None,
            Self::Idle {
                stable_for_ms,
                timeout_ms,
                reaction,
            } => {
                let reaction = reaction.clone().unwrap_or_default();

                ActionCondition::Idle {
                    stable_for: optional_millis(
                        *stable_for_ms,
                        defaults::IDLE_STABLE_FOR,
                        "stable interval",
                    )?,
                    timeout: optional_millis(*timeout_ms, defaults::WAIT_TIMEOUT, "timeout")?,
                    reaction: Reaction {
                        changed_from: reaction.changed_from,
                        timeout: optional_millis(
                            reaction.timeout_ms,
                            defaults::REACTION_TIMEOUT,
                            "reaction timeout",
                        )?,
                    },
                }
            }
            Self::Screen {
                predicate,
                timeout_ms,
            } => ActionCondition::Screen {
                predicate: predicate.clone(),
                timeout: optional_millis(*timeout_ms, defaults::WAIT_TIMEOUT, "timeout")?,
                settle: Duration::ZERO,
            },
        });
    }
}

/// Requested terminating-frame projection in host form.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionSpec {
    /// Requested detail; `text` when absent.
    #[serde(default)]
    pub detail: Option<FrameDetail>,
    /// Exact half-open region; the complete viewport when absent.
    #[serde(default)]
    pub region: Option<ScreenRegion>,
}

impl From<ProjectionSpec> for ScreenProjectionRequest {
    fn from(spec: ProjectionSpec) -> Self {
        return Self {
            detail: spec.detail.unwrap_or_default(),
            region: spec.region,
        };
    }
}

/// One ordered step of [`SendSpec`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
pub enum SendStepSpec {
    /// A run of input atoms applied as one actor-ordered action.
    Input {
        /// Ordered atoms.
        atoms: Vec<AtomSpec>,
    },
    /// Resize the child PTY; pixel dimensions derive from the default cell size.
    Resize {
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
    },
    /// Send one allow-listed signal to the child process group.
    Signal {
        /// Signal to send.
        signal: ProcessSignal,
    },
    /// Append a canonical marker.
    Mark {
        /// Non-empty label.
        label: String,
    },
    /// Apply one cell-addressed pointer operation.
    Pointer {
        /// Zero-based column.
        column: u16,
        /// Zero-based row.
        row: u16,
        /// Click, multi-click, drag, or scroll.
        operation: PointerOperation,
        /// Reject instead of a no-op when the application does not report the pointer.
        #[serde(default)]
        strict: bool,
    },
    /// Report a focus transition.
    Focus {
        /// Whether the terminal gained focus.
        focused: bool,
        /// Reject instead of a no-op when the application does not report focus.
        #[serde(default)]
        strict: bool,
    },
    /// Resolve visible text and click its center.
    Activate {
        /// Exact row-contained visible text.
        text: String,
        /// Require exactly one match.
        #[serde(default)]
        unique: bool,
        /// Reject instead of a no-op when the application does not report the pointer.
        #[serde(default)]
        strict: bool,
    },
}

impl SendStepSpec {
    /// The send step this describes.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for an invalid key chord.
    pub fn to_step(&self) -> Result<SendStep> {
        return Ok(match self {
            Self::Input { atoms } => SendStep::Input(
                atoms
                    .iter()
                    .map(AtomSpec::to_atom)
                    .collect::<Result<Vec<_>>>()?,
            ),
            Self::Resize { cols, rows } => SendStep::Command(ControlCommand::Resize {
                cols: *cols,
                rows: *rows,
                pixel_width: cols.saturating_mul(defaults::CELL_WIDTH),
                pixel_height: rows.saturating_mul(defaults::CELL_HEIGHT),
            }),
            Self::Signal { signal } => {
                SendStep::Command(ControlCommand::Signal { signal: *signal })
            }
            Self::Mark { label } => SendStep::Command(ControlCommand::Mark {
                label: label.clone(),
            }),
            Self::Pointer {
                column,
                row,
                operation,
                strict,
            } => SendStep::Command(ControlCommand::Pointer {
                column: *column,
                row: *row,
                operation: operation.clone(),
                strict: *strict,
            }),
            Self::Focus { focused, strict } => SendStep::Command(ControlCommand::Focus {
                focused: *focused,
                strict: *strict,
            }),
            Self::Activate {
                text,
                unique,
                strict,
            } => SendStep::Command(ControlCommand::Activate {
                text: text.clone(),
                unique: *unique,
                strict: *strict,
            }),
        });
    }
}

/// Ordered mixed steps sent under one caller identity, in host form.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendSpec {
    /// Ordered steps; sending stops at the first failure.
    pub steps: Vec<SendStepSpec>,
    /// Identity of the whole send; later steps derive theirs from it.
    #[serde(default)]
    pub request_id: Option<Uuid>,
    /// Presented-state guard of the first step only.
    #[serde(default)]
    pub if_state: Option<String>,
    /// Human-readable intent retained with input operations.
    #[serde(default)]
    pub intent: Option<String>,
    /// Delay between input atoms.
    #[serde(default)]
    pub pace_ms: Option<u64>,
    /// Report per-atom outcomes of input steps.
    #[serde(default)]
    pub verbose: bool,
    /// Condition checked after the last step.
    #[serde(default)]
    pub until: Option<ConditionSpec>,
    /// Terminating-frame projection.
    #[serde(default)]
    pub projection: Option<ProjectionSpec>,
    /// Report presented-screen changes across the whole send.
    #[serde(default)]
    pub report_changes: bool,
}

impl SendSpec {
    /// The send request this describes, with a fresh identity when none is given.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for an invalid key chord or an out-of-range duration.
    pub fn to_request(&self) -> Result<SendRequest> {
        return Ok(SendRequest {
            steps: self
                .steps
                .iter()
                .map(SendStepSpec::to_step)
                .collect::<Result<Vec<_>>>()?,
            guard: guard(self.request_id, self.if_state.clone()),
            intent: self.intent.clone(),
            pace: optional_millis(self.pace_ms, Duration::ZERO, "pace")?,
            verbose_atom_outcomes: self.verbose,
            postcondition: self
                .until
                .as_ref()
                .map_or(Ok(ActionCondition::None), ConditionSpec::to_condition)?,
            projection: self.projection.unwrap_or_default().into(),
            report_changes: self.report_changes,
        });
    }
}

/// Launch configuration in host form.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchOptions {
    /// Initial terminal rows.
    #[serde(default)]
    pub rows: Option<u16>,
    /// Initial terminal columns.
    #[serde(default)]
    pub cols: Option<u16>,
    /// Child working directory; the caller's when absent.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Complete child-environment policy; inherit everything when absent.
    #[serde(default)]
    pub environment: Option<EnvironmentSpec>,
    /// Recorder readiness deadline.
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    /// Terminate after this many output bytes, retaining all of them.
    #[serde(default)]
    pub output_byte_budget: Option<u64>,
    /// Terminate after this wall-clock duration.
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    /// Terminate after this long without input, output, or control activity.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    /// Visual profile committed into the session; legacy ambient defaults when absent.
    #[serde(default)]
    pub visual_profile: Option<VisualProfile>,
}

impl LaunchOptions {
    /// The launch specification for `command`; the recorder validates it again at start.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for an empty command, zero geometry, an invalid environment,
    /// or a zero resource limit.
    pub fn to_spec(&self, command: Vec<OsString>) -> Result<LaunchSpec> {
        let mut spec = LaunchSpec::new(command).geometry(
            self.cols.unwrap_or(defaults::COLS),
            self.rows.unwrap_or(defaults::ROWS),
        );
        spec.working_directory = self.cwd.as_ref().map(PathBuf::from);

        if let Some(environment) = &self.environment {
            spec.environment = environment.clone();
        }

        if let Some(timeout) = self.startup_timeout_ms {
            spec.startup_timeout = Duration::from_millis(timeout);
        }

        spec.output_byte_budget = self.output_byte_budget;
        spec.max_duration = self.max_duration_ms.map(Duration::from_millis);
        spec.idle_timeout = self.idle_timeout_ms.map(Duration::from_millis);
        spec.visual_profile.clone_from(&self.visual_profile);
        spec.validate()?;
        return Ok(spec);
    }
}

/// Original-capture launch configuration in host form. `grid` and `viewport` are exclusive,
/// and the canonical `rows`/`cols` options do not exist here: the captured terminal owns its grid.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginalLaunchOptions {
    /// Child working directory; the caller's when absent.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Complete child-environment policy; inherit everything when absent.
    #[serde(default)]
    pub environment: Option<EnvironmentSpec>,
    /// Acquisition and recorder startup deadline.
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    /// Terminate after this many output bytes, retaining all of them.
    #[serde(default)]
    pub output_byte_budget: Option<u64>,
    /// Terminate after this wall-clock duration.
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    /// Terminate after this long without input, output, or control activity.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    /// Exact initial grid.
    #[serde(default)]
    pub grid: Option<CellGrid>,
    /// Initial Ghostty window size in pixels; the grid follows its font metrics.
    #[serde(default)]
    pub viewport: Option<PixelCanvas>,
    /// Constant capture canvas; the terminal window when absent.
    #[serde(default)]
    pub canvas: Option<PixelCanvas>,
    /// Acquisition rate: 24, 30 or 60.
    #[serde(default)]
    pub fps: Option<u16>,
    /// Ghostty configuration resolved into the recorded visual profile.
    #[serde(default)]
    pub ghostty_config: Option<String>,
    /// Visual profile with a native section; replaces `ghostty_config`.
    #[serde(default)]
    pub visual_profile: Option<VisualProfile>,
    /// Registry alias bound once the session is live.
    #[serde(default)]
    pub alias: Option<String>,
    /// Registry state root for `alias`.
    #[serde(default)]
    pub state_root: Option<String>,
}

impl OriginalLaunchOptions {
    /// The original launch specification for `command`.
    ///
    /// # Errors
    ///
    /// Returns `invalid_operation` for conflicting geometry, an unsupported rate, or any value
    /// the specification rejects.
    pub fn to_spec(&self, command: Vec<OsString>) -> Result<OriginalLaunchSpec> {
        let mut spec = OriginalLaunchSpec::new(command);
        spec.geometry = match (self.grid, self.viewport) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidOperation {
                    reason: "grid and viewport are exclusive original geometry options".to_owned(),
                    source: None,
                });
            }
            (Some(grid), None) => OriginalGeometry::Grid(grid),
            (None, Some(viewport)) => OriginalGeometry::Viewport {
                width: viewport.width,
                height: viewport.height,
            },
            (None, None) => OriginalGeometry::default(),
        };

        spec.canvas = self.canvas;

        if let Some(fps) = self.fps {
            spec.fps = OriginalFps::new(fps)?;
        }

        spec.working_directory = self.cwd.as_ref().map(PathBuf::from);

        if let Some(environment) = &self.environment {
            spec.environment = environment.clone();
        }

        if let Some(timeout) = self.startup_timeout_ms {
            spec.startup_timeout = Duration::from_millis(timeout);
        }

        spec.output_byte_budget = self.output_byte_budget;
        spec.max_duration = self.max_duration_ms.map(Duration::from_millis);
        spec.idle_timeout = self.idle_timeout_ms.map(Duration::from_millis);
        spec.ghostty_config = self.ghostty_config.as_ref().map(PathBuf::from);
        spec.visual_profile.clone_from(&self.visual_profile);
        spec.alias.clone_from(&self.alias);
        spec.state_root = self.state_root.as_ref().map(PathBuf::from);
        return Ok(spec);
    }
}

/// Postcondition result of one action without its frame, which bindings expose separately.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PostconditionSummary {
    /// No postcondition was requested.
    None {
        /// Whether the action matched.
        matched: bool,
        /// Semantic state identity of the returned frame, when one was returned.
        state_hash: Option<String>,
    },
    /// An idle postcondition was requested.
    Idle {
        /// Whether the screen settled.
        matched: bool,
        /// Settlement classification.
        quality: StableCaptureQuality,
        /// Semantic state identity of the returned frame, when one was returned.
        state_hash: Option<String>,
    },
    /// A screen predicate postcondition was requested.
    Screen {
        /// Whether the predicate matched.
        matched: bool,
        /// Completion classification.
        quality: ScreenPostconditionQuality,
        /// Semantic state identity of the returned frame, when one was returned.
        state_hash: Option<String>,
    },
}

impl From<&ActionPostconditionResult> for PostconditionSummary {
    fn from(result: &ActionPostconditionResult) -> Self {
        return match result {
            ActionPostconditionResult::None {
                matched,
                state_hash,
                ..
            } => Self::None {
                matched: *matched,
                state_hash: state_hash.clone(),
            },
            ActionPostconditionResult::Idle {
                matched,
                quality,
                state_hash,
                ..
            } => Self::Idle {
                matched: *matched,
                quality: *quality,
                state_hash: state_hash.clone(),
            },
            ActionPostconditionResult::Screen {
                matched,
                quality,
                state_hash,
                ..
            } => Self::Screen {
                matched: *matched,
                quality: *quality,
                state_hash: state_hash.clone(),
            },
        };
    }
}

/// Acknowledged or marked control command outcome.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlOutcome {
    /// The actor applied the command.
    Acknowledged(ControlAcknowledgement),
    /// The actor recorded a marker.
    Marked(MarkOutcome),
}

/// Outcome of one control command step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum CommandOutcome {
    /// Acknowledged or marked command.
    Control(ControlOutcome),
    /// Pointer, focus, or activation command.
    Pointer(PointerOutcome),
}

impl CommandOutcome {
    /// Classify the actor's payload for one command.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload is not a command outcome.
    pub fn from_payload(request_id: Uuid, payload: &ControlPayload) -> Result<Self> {
        return match payload {
            ControlPayload::Acknowledged { boundaries } => Ok(Self::Control(
                ControlOutcome::Acknowledged(ControlAcknowledgement {
                    request_id,
                    boundaries: boundaries.clone(),
                }),
            )),
            ControlPayload::Marked {
                label,
                event_seq,
                boundaries,
            } => Ok(Self::Control(ControlOutcome::Marked(MarkOutcome {
                request_id,
                label: label.clone(),
                event_seq: *event_seq,
                boundaries: boundaries.clone(),
            }))),
            payload => Ok(Self::Pointer(PointerOutcome::from_payload(
                request_id,
                payload.clone(),
            )?)),
        };
    }
}

/// Outcome of a stop request with its request identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct StopReport {
    /// Identity of the stop request.
    pub request_id: Uuid,
    /// What the stop observed.
    #[serde(flatten)]
    pub outcome: StopOutcome,
}

/// Classified failure of one step that did not abort its operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ErrorReport {
    /// Stable failure kind.
    pub kind: String,
    /// Stable failure code.
    pub code: String,
    /// Whether the failure applied a mutation.
    pub effect: MutationEffect,
    /// Whether retrying can succeed.
    pub retry: Retry,
    /// Human-readable description.
    pub message: String,
}

impl From<&Error> for ErrorReport {
    fn from(error: &Error) -> Self {
        return Self {
            kind: error.kind().as_str().to_owned(),
            code: error.code().to_owned(),
            effect: error.effect(),
            retry: error.retry(),
            message: error.to_string(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;
    use serde_json::json;
    use termtrace_model::execution::environment::EnvironmentBase;

    #[test]
    fn draft_identity_exists_before_sealing_and_freezes_with_the_request() {
        let mut draft = ActionDraft::new();
        let allocated = draft.request_id();
        assert!(!allocated.is_nil());
        assert!(!draft.is_sealed());

        let chosen = parse_request_id("019fddda-17c8-7000-87da-8785e71ab5e0").unwrap();
        draft.set_request_id(chosen).unwrap();
        draft
            .edit(|request| request.atoms.push(InputAtom::Text("a".to_owned())))
            .unwrap();
        let sealed = draft.seal();
        assert_eq!(sealed.guard.request_id, Some(chosen));
        assert_eq!(draft.seal(), sealed);

        for error in [
            draft
                .edit(|request| request.atoms.push(InputAtom::Text("b".to_owned())))
                .unwrap_err(),
            draft.set_request_id(Uuid::now_v7()).unwrap_err(),
            draft.take_fresh().unwrap_err(),
        ] {
            assert_eq!(error.kind(), ErrorKind::InvalidOperation);
        }

        assert_eq!(draft.request_id(), chosen);
        assert_eq!(draft.seal(), sealed);
    }

    #[test]
    fn legacy_fresh_requests_start_over_with_a_new_identity() {
        let mut draft = ActionDraft::new();
        let first = draft.request_id();
        draft
            .edit(|request| request.atoms.push(InputAtom::Text("a".to_owned())))
            .unwrap();
        let taken = draft.take_fresh().unwrap();
        assert_eq!(taken.guard.request_id, Some(first));
        assert_eq!(taken.atoms.len(), 1);
        assert_ne!(draft.request_id(), first);
        assert!(draft.seal().atoms.is_empty());
    }

    #[test]
    fn request_identities_must_be_hyphenated_and_non_nil() {
        for bad in [
            "",
            "not-a-uuid",
            "00000000-0000-0000-0000-000000000000",
            "019fddda17c8700087da8785e71ab5e0",
            "{019fddda-17c8-7000-87da-8785e71ab5e0}",
            "urn:uuid:019fddda-17c8-7000-87da-8785e71ab5e0",
        ] {
            assert_eq!(
                parse_request_id(bad).unwrap_err().kind(),
                ErrorKind::InvalidOperation,
                "{bad}"
            );
        }
        assert!(parse_request_id("019FDDDA-17C8-7000-87DA-8785E71AB5E0").is_ok());
    }

    #[test]
    fn send_specs_decode_host_steps_and_defaults() {
        let request_id = Uuid::now_v7();
        let spec: SendSpec = serde_json::from_value(json!({
            "steps": [
                {"step": "input", "atoms": [{"text": "a"}, {"bytes": "AP8bgA=="}, {"key": "ctrl+c"}, {"paste": "p"}]},
                {"step": "resize", "cols": 100, "rows": 30},
                {"step": "signal", "signal": "user1"},
                {"step": "mark", "label": "m"},
                {"step": "pointer", "column": 1, "row": 2, "operation": {"kind": "drag", "button": "left", "to_column": 3, "to_row": 4, "steps": 2}},
                {"step": "focus", "focused": true, "strict": true},
                {"step": "activate", "text": "OK", "unique": true}
            ],
            "request_id": request_id,
            "if_state": "hash",
            "pace_ms": 5,
            "until": {"condition": "screen", "predicate": {"predicate": "contains", "text": "x"}, "timeout_ms": 1250},
            "projection": {"detail": "cells"},
            "report_changes": true
        }))
        .unwrap();

        let request = spec.to_request().unwrap();
        assert_eq!(request.guard.request_id, Some(request_id));
        assert_eq!(request.guard.if_state.as_deref(), Some("hash"));
        assert_eq!(request.pace, Duration::from_millis(5));
        assert_eq!(request.projection.detail, FrameDetail::Cells);
        assert!(request.report_changes);
        assert_eq!(
            request.postcondition,
            ActionCondition::Screen {
                predicate: ScreenPredicate::Contains {
                    text: "x".to_owned()
                },
                timeout: Duration::from_millis(1250),
                settle: Duration::ZERO,
            }
        );
        assert_eq!(request.steps.len(), 7);
        assert_eq!(
            request.steps[0],
            SendStep::Input(vec![
                InputAtom::Text("a".to_owned()),
                InputAtom::Bytes(vec![0x00, 0xff, 0x1b, 0x80]),
                InputAtom::Key(parse_key("ctrl+c").unwrap()),
                InputAtom::Paste("p".to_owned()),
            ])
        );
        assert_eq!(
            request.steps[1],
            SendStep::Command(ControlCommand::Resize {
                cols: 100,
                rows: 30,
                pixel_width: 100 * defaults::CELL_WIDTH,
                pixel_height: 30 * defaults::CELL_HEIGHT,
            })
        );

        let fresh = SendSpec {
            steps: vec![SendStepSpec::Mark {
                label: "m".to_owned(),
            }],
            ..SendSpec::default()
        }
        .to_request()
        .unwrap();
        assert!(fresh.guard.request_id.is_some());
        assert_eq!(fresh.postcondition, ActionCondition::None);
    }

    #[test]
    fn send_specs_reject_invalid_steps() {
        for bad in [
            json!({"steps": [{"step": "teleport"}]}),
            json!({"steps": [{"step": "input", "atoms": [{"key": "ctrl+nosuchkey"}]}]}),
            json!({"steps": [{"step": "input", "atoms": [{"bytes": "not base64"}]}]}),
            json!({"steps": [], "extra": 1}),
            json!({"steps": [], "pace_ms": -1}),
            json!({"steps": [], "pace_ms": 1.5}),
            json!({"steps": [], "until": {"condition": "idle", "timeout_ms": 3_600_001}}),
        ] {
            let decoded = serde_json::from_value::<SendSpec>(bad.clone());
            assert!(
                decoded.map_or(true, |spec| return spec.to_request().is_err()),
                "{bad}"
            );
        }
    }

    #[test]
    fn idle_conditions_default_and_carry_reactions() {
        let condition = ConditionSpec::Idle {
            stable_for_ms: None,
            timeout_ms: Some(2000),
            reaction: Some(ReactionSpec {
                changed_from: Some("h".to_owned()),
                timeout_ms: Some(300),
            }),
        }
        .to_condition()
        .unwrap();
        assert_eq!(
            condition,
            ActionCondition::Idle {
                stable_for: defaults::IDLE_STABLE_FOR,
                timeout: Duration::from_secs(2),
                reaction: Reaction {
                    changed_from: Some("h".to_owned()),
                    timeout: Duration::from_millis(300),
                },
            }
        );
    }

    #[test]
    fn launch_options_build_specs_with_environment_and_limits() {
        let options: LaunchOptions = serde_json::from_value(json!({
            "rows": 30,
            "cols": 96,
            "cwd": "/tmp",
            "environment": {"base": "clear", "set": {"TT_SET": "Af8="}, "copy": ["TT_COPY"], "color": "never"},
            "startup_timeout_ms": 1500,
            "output_byte_budget": 18_446_744_073_709_551_615_u64,
            "max_duration_ms": 500,
            "idle_timeout_ms": 250
        }))
        .unwrap();

        let spec = options.to_spec(vec![OsString::from("true")]).unwrap();
        assert_eq!((spec.cols, spec.rows), (96, 30));
        assert_eq!(spec.working_directory, Some(PathBuf::from("/tmp")));
        assert_eq!(spec.environment.base, EnvironmentBase::Clear);
        assert_eq!(
            spec.environment.set.get("TT_SET").map(ByteString::as_bytes),
            Some(&[0x01, 0xff][..])
        );
        assert_eq!(spec.startup_timeout, Duration::from_millis(1500));
        assert_eq!(spec.output_byte_budget, Some(u64::MAX));
        assert_eq!(spec.max_duration, Some(Duration::from_millis(500)));
        assert_eq!(spec.idle_timeout, Some(Duration::from_millis(250)));

        for bad in [
            json!({"max_duration_ms": 0}),
            json!({"idle_timeout_ms": 0}),
            json!({"rows": 0}),
            json!({"environment": {"set": {"TERM": "eA=="}}}),
            json!({"environment": {"set": {"A": "AA=="}}}),
        ] {
            let options: LaunchOptions = serde_json::from_value(bad.clone()).unwrap();
            assert_eq!(
                options
                    .to_spec(vec![OsString::from("true")])
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidOperation,
                "{bad}"
            );
        }

        assert!(LaunchOptions::default().to_spec(Vec::new()).is_err());
        assert!(serde_json::from_value::<LaunchOptions>(json!({"env": {}})).is_err());
    }
}
