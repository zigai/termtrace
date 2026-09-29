//! Versioned, bounded client protocol for controlling a live recording.

pub use crate::terminal::observation::ActorBoundaries;
use crate::{
    execution::{application::ApplicationEvent, event::InputKind},
    protocol::mutation::MutationEffect,
    terminal::{
        screen::{
            FrameDetail, InvalidRegionReason, ScreenFrame, ScreenProjectionRequest, ScreenRegion,
        },
        state::{CursorStyle, KeyModifiers, ModeState, MouseButton, NamedKey},
    },
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// A system monotonic clock bracket around a session-relative clock sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClockSample {
    /// Session-relative time sampled inside the system clock bracket.
    pub session_monotonic_ns: u64,
    /// Midpoint of the system `CLOCK_MONOTONIC` bracket.
    pub system_monotonic_ns: u64,
    /// Maximum error of the midpoint mapping, in nanoseconds.
    pub uncertainty_ns: u64,
}

/// Current control protocol version.
pub const CONTROL_PROTOCOL_VERSION: u32 = 19;
/// Maximum encoded request or response size.
pub const MAX_CONTROL_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
/// Response deadline for commands that do not wait or pace input.
pub const DEFAULT_CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound on inter-atom pacing counted toward a response deadline.
const MAX_INPUT_SEQUENCE_PACING: Duration = Duration::from_secs(60);
/// Slack the actor adds after a waiting command's own timeout.
const ACTOR_WAIT_SLACK: Duration = Duration::from_secs(2);
/// Slack a client adds after a waiting command's own timeout.
///
/// It exceeds [`ACTOR_WAIT_SLACK`] so the actor always answers before a
/// client abandons the exchange and reports delivery as unknown.
const CLIENT_WAIT_SLACK: Duration = Duration::from_secs(3);
const CONTROL_SOCKET_PREFIX: &str = "termtrace-control-";

/// Derive the Linux abstract control-socket name for a recording session.
pub fn control_socket_name(session_id: Uuid) -> String {
    return format!("{CONTROL_SOCKET_PREFIX}{session_id}");
}

/// Whether bytes form one newline-terminated control message within the size limit.
pub fn is_control_frame(encoded: &[u8]) -> bool {
    return encoded.last() == Some(&b'\n')
        && u64::try_from(encoded.len()).unwrap_or(u64::MAX) <= MAX_CONTROL_MESSAGE_BYTES;
}

/// Derive an input atom's operation identity from its request and zero-based index.
pub fn derive_input_operation_id(request_id: Uuid, index: usize) -> Uuid {
    let name = format!("termtrace/input-atom/v1/{request_id}/{index}");

    return Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes());
}

/// Safely supported process-group signals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSignal {
    /// Interrupt the foreground application (`SIGINT`).
    Interrupt,
    /// Request graceful termination (`SIGTERM`).
    Terminate,
    /// Notify the application that its terminal disconnected (`SIGHUP`).
    Hangup,
    /// Application-defined signal one (`SIGUSR1`).
    User1,
    /// Application-defined signal two (`SIGUSR2`).
    User2,
}

/// One high-level cell-addressed pointer operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PointerOperation {
    /// Press and release one button at a cell.
    Click {
        /// Button to click.
        button: MouseButton,
    },
    /// Repeat a complete press/release click a bounded number of times.
    MultiClick {
        /// Button to click.
        button: MouseButton,
        /// Number of complete clicks.
        count: u8,
    },
    /// Press at the command cell, emit motion samples, and release at an end cell.
    Drag {
        /// Button held throughout the drag.
        button: MouseButton,
        /// Zero-based destination column.
        to_column: u16,
        /// Zero-based destination row.
        to_row: u16,
        /// Number of motion samples, including the destination.
        steps: u8,
    },
    /// Emit signed vertical wheel ticks at a cell.
    Scroll {
        /// Negative scrolls up and positive scrolls down; zero is invalid.
        lines: i16,
    },
}

/// Exact predicate evaluated against the latest presented viewport.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "predicate", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenPredicate {
    /// Viewport text contains an exact UTF-8 substring.
    Contains {
        /// Exact UTF-8 substring.
        text: String,
    },
    /// Viewport text excludes an exact UTF-8 substring.
    Absent {
        /// Exact UTF-8 substring.
        text: String,
    },
    /// Viewport text matches a bounded linear-time regular expression.
    MatchesRegex {
        /// Rust-regex pattern, limited by the control service.
        pattern: String,
    },
    /// Viewport text does not match a bounded linear-time regular expression.
    ExcludesRegex {
        /// Rust-regex pattern, limited by the control service.
        pattern: String,
    },
    /// One zero-based row contains exact UTF-8 text.
    RowContains {
        /// Zero-based viewport row.
        row: u16,
        /// Exact UTF-8 substring.
        text: String,
    },
    /// One zero-based row excludes exact UTF-8 text.
    RowAbsent {
        /// Zero-based viewport row.
        row: u16,
        /// Exact UTF-8 substring.
        text: String,
    },
    /// A half-open cell region contains exact UTF-8 text.
    RegionContains {
        /// Region bounds in viewport cells.
        region: ScreenRegion,
        /// Exact UTF-8 substring.
        text: String,
    },
    /// A half-open cell region excludes exact UTF-8 text.
    RegionAbsent {
        /// Region bounds in viewport cells.
        region: ScreenRegion,
        /// Exact UTF-8 substring.
        text: String,
    },
    /// Cursor properties all equal the supplied values.
    Cursor {
        /// Optional zero-based viewport column.
        column: Option<u16>,
        /// Optional zero-based viewport row.
        row: Option<u16>,
        /// Optional cursor visibility requirement.
        visible: Option<bool>,
        /// Optional cursor shape requirement.
        shape: Option<CursorStyle>,
    },
    /// One stable terminal mode equals the requested value.
    Mode {
        /// Stable mode selector.
        mode: ScreenMode,
        /// Required set/reset value.
        enabled: bool,
    },
    /// Presented state hash equals or differs from a supplied BLAKE3 hash.
    StateHash {
        /// Lowercase BLAKE3 state hash.
        hash: String,
        /// Equality relationship to require.
        comparison: StateHashComparison,
    },
    /// Every nested predicate must match.
    All {
        /// Bounded non-empty predicate list.
        predicates: Vec<ScreenPredicate>,
    },
    /// At least one nested predicate must match.
    Any {
        /// Bounded non-empty predicate list.
        predicates: Vec<ScreenPredicate>,
    },
}

/// Stable terminal-mode selectors available to screen predicates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenMode {
    /// Any alternate-screen mode.
    AlternateScreen,
    /// DEC bracketed-paste mode 2004.
    BracketedPaste,
    /// DEC auto-wrap mode 7.
    Wraparound,
    /// DEC synchronized-output mode 2026.
    SynchronizedOutput,
    /// Numeric ANSI mode.
    Ansi {
        /// Mode number.
        number: u16,
    },
    /// Numeric DEC-private mode.
    Dec {
        /// Mode number.
        number: u16,
    },
}

/// Required relationship between the presented state and a supplied hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateHashComparison {
    /// Current state must equal the supplied hash.
    Unchanged,
    /// Current state must differ from the supplied hash.
    Changed,
}

/// Why an atomic stable-screen capture returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StableCaptureQuality {
    /// The exact presented state remained unchanged for the requested interval.
    Stable,
    /// The child completed before the requested stable interval elapsed.
    ChildCompleted,
    /// The PTY output stream closed before the requested stable interval elapsed.
    OutputClosed,
    /// The server-side deadline elapsed before the requested stable interval.
    Deadline,
    /// The session had already completed; the frame is its final presented state.
    Completed,
}

impl StableCaptureQuality {
    /// Stable serialized spelling for diagnostics and APIs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        return match self {
            Self::Stable => "stable",
            Self::ChildCompleted => "child_completed",
            Self::OutputClosed => "output_closed",
            Self::Deadline => "deadline",
            Self::Completed => "completed",
        };
    }

    /// Whether the returned frame can no longer change: stable while live, or final.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        return matches!(self, Self::Stable | Self::Completed);
    }
}

/// Why an exact actor-side screen predicate operation returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenWaitQuality {
    /// The predicate matched and its state met the requested stability interval.
    Matched,
    /// The predicate matched but no stability interval was requested.
    MatchedUnsettled,
    /// Child completion terminated the operation before a match.
    ChildCompleted,
    /// PTY output closure terminated the operation before a match.
    OutputClosed,
    /// The actor-side deadline elapsed before a match.
    Deadline,
    /// The session had already completed; the predicate was evaluated on its final screen.
    Completed,
}

/// Why an exact action screen postcondition returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenPostconditionQuality {
    /// The predicate matched and its state met the requested stability contract.
    Matched,
    /// The predicate matched without an additional settling interval.
    MatchedUnsettled,
    /// Child completion terminated the postcondition before a match.
    ChildCompleted,
    /// PTY output closure terminated the postcondition before a match.
    OutputClosed,
    /// The actor-side deadline elapsed before a match.
    Deadline,
}

/// One command sent to the live session actor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    /// Verify that the host and protocol are responsive.
    Ping,
    /// Return current process and event counters.
    Status,
    /// Return actor-owned durable event-file positions for incremental following.
    EventCursor {
        /// Requested next event; absent from retention when explicit historical reading is needed.
        next_event_seq: u64,
    },
    /// Return the latest presentation-eligible semantic viewport.
    Snapshot,
    /// Return one bounded rich projection of the latest presented state.
    ScreenFrame {
        /// Requested rich detail and exact optional viewport region.
        projection: ScreenProjectionRequest,
    },
    /// Wait inside the actor for a stable presentation-eligible predicate.
    WaitScreen {
        /// Exact screen predicate.
        predicate: ScreenPredicate,
        /// Required unchanged matching-state interval.
        stable_for_ns: u64,
        /// Server-side monotonic deadline interval.
        timeout_ns: u64,
        /// Rich projection returned from the exact satisfying or terminating state.
        projection: ScreenProjectionRequest,
    },
    /// Atomically wait for a stable presented state and return that exact actor snapshot.
    CaptureStable {
        /// Required unchanged presented-state interval.
        stable_for_ns: u64,
        /// Server-side monotonic deadline interval.
        timeout_ns: u64,
        /// Rich projection returned from the exact terminating state.
        projection: ScreenProjectionRequest,
    },
    /// Send exact bytes to the child PTY.
    Input {
        /// Raw bytes encoded as Base64 in JSON.
        data: ByteString,
        /// Semantic category retained in the canonical input intent.
        input_kind: InputKind,
        /// Optional caller-supplied reason for the operation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_intent: Option<String>,
    },
    /// Apply one admission-guarded, journaled sequence of keyboard input atoms.
    InputSequence {
        /// Ordered input atoms encoded against actor state immediately before each write.
        atoms: Vec<InputSequenceAtom>,
        /// Delay between atoms, bounded together with the atom count.
        pace_ns: u64,
        /// Retain per-atom identifiers and canonical event positions in the response.
        include_atom_outcomes: bool,
    },
    /// Apply input and evaluate its postcondition as one actor-ordered operation.
    Act {
        /// Ordered input atoms encoded against actor state immediately before each write.
        atoms: Vec<InputSequenceAtom>,
        /// Delay between atoms, bounded together with the atom count.
        pace_ns: u64,
        /// Retain per-atom identifiers and canonical event positions in the response.
        include_atom_outcomes: bool,
        /// Presented-state condition evaluated without releasing actor ordering.
        postcondition: ActionPostcondition,
        /// Rich projection returned from the exact terminating state.
        projection: ScreenProjectionRequest,
    },
    /// Encode and apply one symbolic key against the actor's live terminal modes.
    Key {
        /// Layout-independent key.
        key: NamedKey,
        /// Typed modifier state.
        modifiers: KeyModifiers,
    },
    /// Paste UTF-8 bytes using the actor's live bracketed-paste mode.
    Paste {
        /// Paste content before mode-dependent wrapping or newline conversion.
        data: ByteString,
    },
    /// Report a terminal-window focus transition when the application opted in.
    Focus {
        /// Whether the terminal gained (`true`) or lost (`false`) focus.
        focused: bool,
        /// Reject instead of returning a semantic no-op when reporting is disabled.
        strict: bool,
    },
    /// Encode one cell-addressed pointer operation from live mouse modes.
    Pointer {
        /// Zero-based viewport column.
        column: u16,
        /// Zero-based viewport row.
        row: u16,
        /// Click or vertical scrolling plan.
        operation: PointerOperation,
        /// Reject instead of returning a semantic no-op when reporting is disabled.
        strict: bool,
    },
    /// Resolve visible text and click its center from one actor-ordered state.
    Activate {
        /// Exact row-contained visible text to resolve.
        text: String,
        /// Require exactly one match instead of choosing the first.
        unique: bool,
        /// Reject instead of returning a semantic no-op when reporting is disabled.
        strict: bool,
    },
    /// Change the child PTY geometry.
    Resize {
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
        /// Total pixel width.
        pixel_width: u16,
        /// Total pixel height.
        pixel_height: u16,
    },
    /// Send one allow-listed signal to the child process group.
    Signal {
        /// Signal to send.
        signal: ProcessSignal,
    },
    /// Add a named point to the canonical event stream.
    Mark {
        /// Non-empty marker label.
        label: String,
    },
    /// Append application instrumentation independently of PTY traffic.
    Application {
        /// Bounded caller-reported diagnostic record.
        data: ApplicationEvent,
    },
    /// Request graceful child termination.
    Stop,
}

impl ControlCommand {
    /// Whether the command may change the child, its PTY, or canonical evidence.
    ///
    /// Mutations are journaled for idempotent replay, accept presented-state
    /// guards, and have uncertain effects when delivery fails after dispatch.
    pub const fn is_mutation(&self) -> bool {
        return match self {
            Self::Ping
            | Self::Status
            | Self::EventCursor { .. }
            | Self::Snapshot
            | Self::ScreenFrame { .. }
            | Self::WaitScreen { .. }
            | Self::CaptureStable { .. } => false,
            Self::Input { .. }
            | Self::InputSequence { .. }
            | Self::Act { .. }
            | Self::Key { .. }
            | Self::Paste { .. }
            | Self::Focus { .. }
            | Self::Pointer { .. }
            | Self::Activate { .. }
            | Self::Resize { .. }
            | Self::Signal { .. }
            | Self::Mark { .. }
            | Self::Application { .. }
            | Self::Stop => true,
        };
    }

    /// Effect a caller must assume when this command reached the actor but no
    /// response came back.
    pub const fn unanswered_effect(&self) -> MutationEffect {
        if self.is_mutation() {
            return MutationEffect::Unknown;
        }
        return MutationEffect::NotApplied;
    }

    /// How long the actor may take to answer before it abandons the request.
    pub fn actor_response_timeout(&self) -> Duration {
        return self.response_timeout(ACTOR_WAIT_SLACK);
    }

    /// How long a client waits for the answer; never shorter than the actor's deadline.
    pub fn client_response_timeout(&self) -> Duration {
        return self.response_timeout(CLIENT_WAIT_SLACK);
    }

    fn response_timeout(&self, wait_slack: Duration) -> Duration {
        return match self {
            Self::WaitScreen { timeout_ns, .. } | Self::CaptureStable { timeout_ns, .. } => {
                Duration::from_nanos(*timeout_ns).saturating_add(wait_slack)
            }
            Self::InputSequence { atoms, pace_ns, .. } => {
                input_sequence_response_timeout(atoms, *pace_ns)
            }
            Self::Act {
                atoms,
                pace_ns,
                postcondition,
                ..
            } => {
                let postcondition_timeout = match postcondition {
                    ActionPostcondition::None => Duration::ZERO,
                    ActionPostcondition::Idle { timeout_ns, .. }
                    | ActionPostcondition::Screen { timeout_ns, .. } => {
                        Duration::from_nanos(*timeout_ns)
                    }
                };

                input_sequence_response_timeout(atoms, *pace_ns)
                    .saturating_add(postcondition_timeout)
                    .saturating_add(wait_slack)
            }
            Self::Ping
            | Self::Status
            | Self::EventCursor { .. }
            | Self::Snapshot
            | Self::ScreenFrame { .. }
            | Self::Input { .. }
            | Self::Key { .. }
            | Self::Paste { .. }
            | Self::Focus { .. }
            | Self::Pointer { .. }
            | Self::Activate { .. }
            | Self::Resize { .. }
            | Self::Signal { .. }
            | Self::Mark { .. }
            | Self::Application { .. }
            | Self::Stop => DEFAULT_CONTROL_RESPONSE_TIMEOUT,
        };
    }
}

fn input_sequence_response_timeout(atoms: &[InputSequenceAtom], pace_ns: u64) -> Duration {
    let gaps = u64::try_from(atoms.len().saturating_sub(1)).unwrap_or(u64::MAX);
    let pacing = Duration::from_nanos(pace_ns.saturating_mul(gaps)).min(MAX_INPUT_SEQUENCE_PACING);

    return DEFAULT_CONTROL_RESPONSE_TIMEOUT.saturating_add(pacing);
}

/// Request envelope carried as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    /// Protocol version.
    pub version: u32,
    /// Caller-generated correlation identifier.
    pub request_id: Uuid,
    /// Optional optimistic guard over the actor's latest presented state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_presented_state_hash: Option<String>,
    /// Requested operation.
    pub command: ControlCommand,
}

impl ControlRequest {
    /// Construct a request with a fresh time-sortable identifier.
    pub fn new(command: ControlCommand) -> Self {
        return Self {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::now_v7(),
            expected_presented_state_hash: None,
            command,
        };
    }

    /// Require the mutation to observe this presented-state hash.
    #[must_use]
    pub fn with_expected_presented_state_hash(mut self, hash: impl Into<String>) -> Self {
        self.expected_presented_state_hash = Some(hash.into());

        return self;
    }
}

/// An actor-owned position immediately after a canonical record (or the header).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiveEventCursor {
    /// Sequence of the next record.
    pub next_event_seq: u64,
    /// Byte offset of the next record.
    pub file_offset: u64,
    /// Hash of the preceding record or header.
    pub event_hash: String,
    /// Timestamp of the preceding record, or zero at the header.
    pub monotonic_ns: u64,
}

/// Successful response payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlData {
    /// Read-only actor authority for a durable event-file suffix.
    EventCursor {
        /// Device of the writer's open file descriptor.
        device: u64,
        /// Inode of the writer's open file descriptor.
        inode: u64,
        /// Canonical header hash retained by the writer.
        header_hash: String,
        /// Requested position, if still retained and durable.
        start: Option<LiveEventCursor>,
        /// Latest data-synchronized position.
        durable: LiveEventCursor,
    },
    /// No additional data is required.
    Acknowledged {
        /// Durable and observed canonical positions after the operation.
        boundaries: ActorBoundaries,
    },
    /// Exact result of one applied input operation.
    InputApplied {
        /// Canonical operation identifier (the caller request ID).
        operation_id: Uuid,
        /// Semantic input category.
        input_kind: InputKind,
        /// Exact bytes emitted by the actor.
        data: ByteString,
        /// Number of bytes written completely.
        bytes_written: u64,
        /// Canonical intent event sequence.
        intent_event_seq: u64,
        /// Canonical result event sequence.
        result_event_seq: u64,
        /// Durable and observed positions after application.
        boundaries: ActorBoundaries,
    },
    /// Complete result of one admission-guarded input sequence.
    InputSequenceApplied {
        /// Canonical sequence identifier (the caller request ID).
        sequence_id: Uuid,
        /// Number of atoms applied completely.
        atom_count: u32,
        /// Total exact PTY bytes written across every atom.
        bytes_written: u64,
        /// First canonical input-intent event in the sequence.
        first_intent_event_seq: u64,
        /// Last canonical input-result event in the sequence.
        last_result_event_seq: u64,
        /// Requested delay between atoms.
        pace_ns: u64,
        /// Optional per-atom outcomes requested by an explicit verbose caller.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        atoms: Vec<InputSequenceAtomOutcome>,
        /// Durable and observed positions after complete application.
        boundaries: ActorBoundaries,
    },
    /// Combined input and postcondition result from one actor-ordered action.
    ActionApplied {
        /// Exact result of the completely applied input prefix.
        input: ActionInputResult,
        /// Exact presented state that satisfied or ended the postcondition.
        postcondition: ActionPostconditionResult,
        /// Durable and observed positions after postcondition evaluation.
        boundaries: ActorBoundaries,
    },
    /// Successful semantic input request that correctly emitted no bytes.
    InputNoop {
        /// Semantic input category.
        input_kind: InputKind,
        /// Stable explanation for the no-op.
        reason: String,
        /// BLAKE3 of canonical JSON for the terminal mode snapshot.
        terminal_modes_hash: String,
        /// Durable and observed positions at evaluation.
        boundaries: ActorBoundaries,
    },
    /// Exact result of one semantic text activation.
    Activated {
        /// Canonical operation identifier.
        operation_id: Uuid,
        /// Exact matched text.
        text: String,
        /// Zero-based first matched column.
        start_column: u16,
        /// Zero-based exclusive matched column.
        end_column: u16,
        /// Zero-based matched row.
        row: u16,
        /// Cell chosen for the click.
        target_column: u16,
        /// Presented-state hash used for resolution and encoding.
        presented_state_hash: String,
        /// Exact bytes emitted by the actor.
        data: ByteString,
        /// Number of bytes written completely.
        bytes_written: u64,
        /// Canonical intent event sequence.
        intent_event_seq: u64,
        /// Canonical result event sequence.
        result_event_seq: u64,
        /// Durable and observed positions after application.
        boundaries: ActorBoundaries,
    },
    /// Live session status.
    Status {
        /// Child process identifier when available.
        pid: Option<u32>,
        /// Durable and observed canonical positions at status evaluation.
        boundaries: ActorBoundaries,
        /// Bounded mapping between session-relative and system monotonic time.
        clock_sample: ClockSample,
    },
    /// Observer-only latest presented terminal state.
    Snapshot {
        /// Zstandard-compressed canonical JSON for the complete semantic state.
        semantic_state_zstd: ByteString,
        /// Plain text of the presentation-eligible viewport.
        viewport_text: String,
        /// Normalized semantic hash assigned by deterministic replay.
        state_hash: String,
        /// Always true for this presentation-filtered endpoint.
        presented: bool,
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
        /// Cursor column when it lies in the viewport.
        cursor_column: Option<u16>,
        /// Cursor row when it lies in the viewport.
        cursor_row: Option<u16>,
        /// Whether the cursor is visible.
        cursor_visible: bool,
        /// Session-monotonic observation timestamp.
        observed_monotonic_ns: u64,
        /// Presentation-eligible terminal modes.
        modes: ModeState,
        /// Durable and observed source boundaries at observation time.
        boundaries: ActorBoundaries,
    },
    /// Bounded rich projection of one exact actor-presented state.
    ScreenFrame {
        /// Exact owned projection produced before actor ordering was released.
        frame: ScreenFrame,
    },
    /// Atomic stable-screen wait result and its exact terminating frame.
    StableSnapshot {
        /// Exact frame projected before waiter ordering was released.
        frame: ScreenFrame,
        /// Zstandard-compressed canonical JSON for this exact semantic state.
        semantic_state_zstd: ByteString,

        /// First actor timestamp at which this exact state was observed.
        first_observed_monotonic_ns: u64,
        /// Actor timestamp of the returned snapshot.
        observed_monotonic_ns: u64,
        /// Requested unchanged interval.
        stable_for_ns: u64,
        /// Typed reason the operation returned.
        quality: StableCaptureQuality,
    },
    /// Exact result of one server-side presented-screen wait.
    ScreenWait {
        /// Predicate that was evaluated.
        predicate: ScreenPredicate,
        /// Whether the predicate matched before termination.
        matched: bool,
        /// First matching timestamp for this unchanged state, when matched.
        first_match_monotonic_ns: Option<u64>,
        /// Last actor timestamp at which the returned state was evaluated.
        observed_monotonic_ns: u64,
        /// Requested unchanged interval.
        stable_for_ns: u64,
        /// Closed completion classification.
        quality: ScreenWaitQuality,
        /// Exact satisfying or terminating frame.
        frame: ScreenFrame,
    },
    /// Canonical position of an appended marker.
    Marked {
        /// Normalized marker label.
        label: String,
        /// Canonical event sequence containing the marker.
        event_seq: u64,
        /// Durable and observed canonical positions after the marker.
        boundaries: ActorBoundaries,
    },
}

/// Presented-state condition retained under exclusive actor ordering after input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "condition", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionPostcondition {
    /// Return immediately after the input sequence completes.
    None,
    /// Wait for one unchanged presentation-eligible state.
    Idle {
        /// Required unchanged presented-state interval.
        stable_for_ns: u64,
        /// Server-side monotonic deadline interval.
        timeout_ns: u64,
    },
    /// Wait for one presentation-eligible screen predicate.
    Screen {
        /// Exact screen predicate.
        predicate: ScreenPredicate,
        /// Server-side monotonic deadline interval.
        timeout_ns: u64,
    },
}

/// Exact applied-input component of an actor-ordered action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionInputResult {
    /// Canonical sequence identifier (the action request ID).
    pub sequence_id: Uuid,
    /// Number of atoms applied completely.
    pub atom_count: u32,
    /// Total exact PTY bytes written across every atom.
    pub bytes_written: u64,
    /// First canonical input-intent event in the sequence.
    pub first_intent_event_seq: u64,
    /// Last canonical input-result event in the sequence.
    pub last_result_event_seq: u64,
    /// Requested delay between atoms.
    pub pace_ns: u64,
    /// Optional per-atom outcomes requested by an explicit verbose caller.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub atoms: Vec<InputSequenceAtomOutcome>,
}

/// Exact postcondition result returned without releasing actor ordering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionPostconditionResult {
    /// No presented-state postcondition was requested.
    None {
        /// Always true after complete input application.
        matched: bool,
        /// Exact terminating state hash when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_hash: Option<String>,
        /// Exact immediate post-input frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<ScreenFrame>,
    },
    /// Final state of an idle postcondition.
    Idle {
        /// Whether the state remained unchanged for the requested interval.
        matched: bool,
        /// Typed stability quality.
        quality: StableCaptureQuality,
        /// Exact terminating state hash when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_hash: Option<String>,
        /// Exact terminating frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<ScreenFrame>,
    },
    /// Final state of a screen postcondition.
    Screen {
        /// Whether the predicate matched before termination.
        matched: bool,
        /// Closed completion classification.
        quality: ScreenPostconditionQuality,
        /// Exact terminating state hash when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_hash: Option<String>,
        /// Exact terminating frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<ScreenFrame>,
    },
}

/// One input atom accepted inside an admission-guarded sequence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "input", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputSequenceAtom {
    /// Exact caller bytes with an explicit semantic category.
    Bytes {
        /// Caller-owned operation identifier retained in canonical events.
        operation_id: Uuid,
        /// Bytes encoded as Base64 in JSON.
        data: ByteString,
        /// Semantic category retained in canonical evidence.
        input_kind: InputKind,
        /// Optional caller-supplied reason retained separately from input semantics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_intent: Option<String>,
    },
    /// One symbolic key encoded against live terminal modes.
    Key {
        /// Caller-owned operation identifier retained in canonical events.
        operation_id: Uuid,
        /// Layout-independent key.
        key: NamedKey,
        /// Typed modifier state.
        modifiers: KeyModifiers,
        /// Optional caller-supplied reason retained separately from the symbolic key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_intent: Option<String>,
    },
    /// UTF-8 paste encoded using live bracketed-paste mode.
    Paste {
        /// Caller-owned operation identifier retained in canonical events.
        operation_id: Uuid,
        /// Paste content before mode-dependent encoding.
        data: ByteString,
        /// Optional caller-supplied reason retained separately from paste semantics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_intent: Option<String>,
    },
}

/// Canonical result coordinates for one atom in a successful input sequence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputSequenceAtomOutcome {
    /// Caller-owned operation identifier for this atom.
    pub operation_id: Uuid,
    /// Semantic category retained in canonical evidence.
    pub input_kind: InputKind,
    /// Exact number of PTY bytes written.
    pub bytes_written: u64,
    /// Canonical input-intent event sequence.
    pub intent_event_seq: u64,
    /// Canonical input-result event sequence.
    pub result_event_seq: u64,
}

/// Response envelope carried as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlResponse {
    /// Protocol version.
    pub version: u32,
    /// Identifier copied from the request.
    pub request_id: Uuid,
    /// Success or structured rejection.
    pub result: ControlResult,
}

/// Successful response or stable rejection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResult {
    /// Command was accepted and completely applied.
    Ok {
        /// Command-specific data.
        data: ControlData,
    },
    /// Command was rejected or failed.
    Error {
        /// Stable machine-readable code.
        code: String,
        /// Human-readable diagnostic.
        message: String,
        /// What the request did before it failed, as stated by the actor.
        effect: MutationEffect,
        /// Actor-consistent context for a rejected optimistic guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<ControlErrorDetails>,
    },
}

/// Structured actor context attached to a rejected or incomplete request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlErrorDetails {
    /// Latest presented-state hash at the actor ordering point.
    pub current_presented_state_hash: String,
    /// Durable and observed source positions at the failure boundary.
    pub boundaries: ActorBoundaries,
    /// Exact output or mutation scope covered by these details.
    pub scope: String,
    /// Exact attempted prefix when an admitted input sequence stopped early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_sequence: Option<InputSequenceFailureDetails>,
    /// Bounded rich-projection validation or resource-limit facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_frame: Option<ScreenFrameFailureDetails>,
}

/// Bounded context for a rejected rich screen projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenFrameFailureDetails {
    /// Requested projection detail when known.
    pub detail: Option<FrameDetail>,
    /// Requested region when supplied.
    pub region: Option<ScreenRegion>,
    /// Typed invalid-region reason when validation failed.
    pub region_reason: Option<InvalidRegionReason>,
    /// Full terminal columns when geometry was available.
    pub cols: Option<u16>,
    /// Full terminal rows when geometry was available.
    pub rows: Option<u16>,
    /// `cell_count`, `serialized_bytes`, or `response_bytes` for limit failures.
    pub limit_kind: Option<String>,
    /// Requested measured amount for a limit failure.
    pub requested: Option<u64>,
    /// Maximum permitted amount for a limit failure.
    pub maximum: Option<u64>,
}

/// Recoverable accounting for an input sequence that stopped during PTY delivery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputSequenceFailureDetails {
    /// Caller-owned sequence request identifier.
    pub sequence_id: Uuid,
    /// Total number of requested atoms.
    pub atom_count: u32,
    /// Number of preceding atoms applied completely.
    pub atoms_applied: u32,
    /// Zero-based atom whose write failed.
    pub failed_atom_index: u32,
    /// Caller-owned operation identifier for the failed atom.
    pub failed_operation_id: Uuid,
    /// Total bytes written across all attempted atoms.
    pub bytes_written: u64,
    /// Bytes written for the failed atom; nonzero means application may be partial.
    pub failed_atom_bytes_written: u64,
    /// First canonical input-intent event in the attempted prefix.
    pub first_intent_event_seq: u64,
    /// Canonical failed input-result event.
    pub last_result_event_seq: u64,
    /// First atom safe to submit independently with a new request ID.
    pub resume_from_atom_index: u32,
}

impl InputSequenceFailureDetails {
    /// Effect of the attempted prefix these details describe.
    #[must_use]
    pub const fn effect(&self) -> MutationEffect {
        return MutationEffect::stopped_input_sequence(
            self.atoms_applied,
            self.failed_atom_bytes_written,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_schema_rejects_unknown_fields() {
        let request = ControlRequest::new(ControlCommand::Ping);
        let mut encoded = serde_json::to_value(&request).expect("ping request serializes");
        let decoded = serde_json::from_value::<ControlRequest>(encoded.clone())
            .expect("unmodified ping request is accepted");
        assert_eq!(decoded, request);

        encoded
            .as_object_mut()
            .expect("control request serializes as an object")
            .insert("extra".to_owned(), serde_json::Value::Bool(true));

        assert!(serde_json::from_value::<ControlRequest>(encoded).is_err());
    }

    #[test]
    fn rejects_unterminated_and_oversized_frames() {
        assert!(is_control_frame(b"{}\n"));
        assert!(!is_control_frame(b""));
        assert!(!is_control_frame(br#"{"version":1}"#));

        let mut oversized = vec![b'x'; usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap()];
        oversized.push(b'\n');

        assert!(!is_control_frame(&oversized));
    }

    fn two_text_atoms() -> Vec<InputSequenceAtom> {
        return (0..2)
            .map(|index| {
                return InputSequenceAtom::Bytes {
                    operation_id: derive_input_operation_id(Uuid::nil(), index),
                    data: ByteString::new(vec![b'a']),
                    input_kind: InputKind::Text,
                    caller_intent: None,
                };
            })
            .collect();
    }

    #[test]
    fn paced_sequences_extend_both_sides_of_the_control_deadline() {
        let command = ControlCommand::InputSequence {
            atoms: two_text_atoms(),
            pace_ns: 11_000_000_000,
            include_atom_outcomes: false,
        };
        assert_eq!(command.actor_response_timeout(), Duration::from_secs(21));
        assert_eq!(command.client_response_timeout(), Duration::from_secs(21));
    }

    #[test]
    fn clients_outwait_the_actor_for_waiting_commands() {
        let wait = ControlCommand::WaitScreen {
            predicate: ScreenPredicate::Contains {
                text: "ready".to_owned(),
            },
            stable_for_ns: 0,
            timeout_ns: 5_000_000_000,
            projection: ScreenProjectionRequest::default(),
        };

        let act = ControlCommand::Act {
            atoms: two_text_atoms(),
            pace_ns: 0,
            include_atom_outcomes: false,
            postcondition: ActionPostcondition::None,
            projection: ScreenProjectionRequest::default(),
        };

        for command in [wait, act] {
            assert!(command.client_response_timeout() > command.actor_response_timeout());
        }

        assert_eq!(
            ControlCommand::Ping.client_response_timeout(),
            DEFAULT_CONTROL_RESPONSE_TIMEOUT
        );
    }

    #[test]
    fn only_observations_are_not_mutations() {
        assert!(!ControlCommand::Snapshot.is_mutation());
        assert!(!ControlCommand::Status.is_mutation());
        assert!(ControlCommand::Stop.is_mutation());
        assert!(ControlCommand::Mark {
            label: "checkpoint".to_owned()
        }
        .is_mutation());
    }

    #[test]
    fn unanswered_mutations_have_unknown_effect() {
        assert_eq!(
            ControlCommand::Snapshot.unanswered_effect(),
            MutationEffect::NotApplied
        );
        assert_eq!(
            ControlCommand::Stop.unanswered_effect(),
            MutationEffect::Unknown
        );
    }

    #[test]
    fn stopped_sequence_details_state_their_effect() {
        let mut details = InputSequenceFailureDetails {
            sequence_id: Uuid::nil(),
            atom_count: 3,
            atoms_applied: 2,
            failed_atom_index: 2,
            failed_operation_id: Uuid::nil(),
            bytes_written: 2,
            failed_atom_bytes_written: 0,
            first_intent_event_seq: 1,
            last_result_event_seq: 6,
            resume_from_atom_index: 3,
        };
        assert_eq!(
            details.effect(),
            MutationEffect::Partial { completed_atoms: 2 }
        );
        details.failed_atom_bytes_written = 1;
        assert_eq!(details.effect(), MutationEffect::Unknown);
    }
}
