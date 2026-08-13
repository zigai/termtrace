//! Versioned, bounded client protocol for controlling a live recording.

use std::{
    io::{BufRead as _, BufReader, Error as IoError, ErrorKind, Read as _, Write as _},
    net::Shutdown,
    os::{
        linux::net::SocketAddrExt as _,
        unix::net::{SocketAddr, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use nix::{
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::getuid,
};
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::screen_frame::ScreenRegion;
use crate::{
    Error, Result,
    event::{ByteString, InputKind},
    event_log::CanonicalBoundary,
    ghostty_vt::{CursorStyle, KeyModifiers, ModeState, MouseButton, NamedKey},
    screen_frame::{FrameDetail, InvalidRegionReason, ScreenFrame, ScreenProjectionRequest},
    session::read_manifest,
};

/// Current control protocol version.
pub const CONTROL_PROTOCOL_VERSION: u32 = 15;
/// Prefix for the Linux abstract socket name assigned to each live session.
pub const CONTROL_SOCKET_PREFIX: &str = "termtrace-control-";
/// Maximum encoded request or response size.
pub const MAX_CONTROL_MESSAGE_BYTES: u64 = 1024 * 1024;
const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INPUT_SEQUENCE_PACING: Duration = Duration::from_secs(60);

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
        };
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
    /// Request graceful child termination.
    Stop,
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

/// Successful response payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlData {
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
        /// Exact immediate post-input frame.
        frame: ScreenFrame,
    },
    /// Final state of an idle postcondition.
    Idle {
        /// Whether the state remained unchanged for the requested interval.
        matched: bool,
        /// Typed stability quality.
        quality: StableCaptureQuality,
        /// Exact terminating frame.
        frame: ScreenFrame,
    },
    /// Final state of a screen postcondition.
    Screen {
        /// Whether the predicate matched before termination.
        matched: bool,
        /// Closed completion classification.
        quality: ScreenPostconditionQuality,
        /// Exact terminating frame.
        frame: ScreenFrame,
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

/// Durable and latest actor-processed positions returned by live operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorBoundaries {
    /// Latest boundary whose file synchronization completed successfully.
    pub durable: CanonicalBoundary,
    /// Latest boundary processed by the session actor, possibly volatile.
    pub observed: CanonicalBoundary,
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

/// Derive the bounded abstract control-socket name for one session.
pub fn control_socket_name(session_id: Uuid) -> String {
    return format!("{CONTROL_SOCKET_PREFIX}{session_id}");
}

pub(crate) fn control_socket_address(name: &str) -> Result<SocketAddr> {
    return SocketAddr::from_abstract_name(name.as_bytes()).map_err(|error| {
        return Error::InvalidControlProtocol {
            reason: format!("invalid abstract control socket name: {error}"),
        };
    });
}

pub(crate) fn connect_session_control(session_directory: &Path) -> Result<UnixStream> {
    if !session_directory.join("session.json").is_file() {
        return Err(Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: control_lifecycle_hint(session_directory),
            source: IoError::new(ErrorKind::NotFound, "session manifest is unavailable"),
        });
    }
    let manifest = read_manifest(session_directory)?;
    let address = control_socket_address(&manifest.control_socket)?;
    let stream = UnixStream::connect_addr(&address).map_err(|source| {
        return Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: control_lifecycle_hint(session_directory),
            source,
        };
    })?;
    let credentials = getsockopt(&stream, PeerCredentials).map_err(|error| {
        return Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: "control_peer_identity_unavailable".to_owned(),
            source: IoError::from_raw_os_error(error as i32),
        };
    })?;
    if credentials.uid() != getuid().as_raw() {
        return Err(Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: "control_peer_owner_mismatch".to_owned(),
            source: IoError::new(
                ErrorKind::PermissionDenied,
                "control endpoint belongs to a different user",
            ),
        });
    }
    return Ok(stream);
}
/// Return whether the live recorder's abstract control endpoint accepts a connection.
pub fn control_endpoint_is_reachable(session_directory: impl AsRef<Path>) -> bool {
    return connect_session_control(session_directory.as_ref()).is_ok();
}

/// Send one command to a live session and wait for its correlated response.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, correlation, or
/// remote operation failures.
pub fn request(
    session_directory: impl AsRef<Path>,
    command: ControlCommand,
) -> Result<ControlData> {
    return send_request(session_directory, &ControlRequest::new(command));
}

/// Send a caller-owned request, preserving its identifier for safe retries.
///
/// Reusing the same request cannot apply a mutation twice; reusing its
/// identifier for a different command is rejected.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, correlation, or
/// remote operation failures.
pub fn send_request(
    session_directory: impl AsRef<Path>,
    request: &ControlRequest,
) -> Result<ControlData> {
    let response = send_request_response(session_directory, request)?;
    match response.result {
        ControlResult::Ok { data } => return Ok(data),
        ControlResult::Error { code, message, .. } => {
            return Err(Error::ControlRejected { code, message });
        }
    }
}

/// Send a caller-owned request and retain structured success or rejection data.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, or correlation
/// failures. Actor rejections are returned in the response envelope.
pub fn send_request_response(
    session_directory: impl AsRef<Path>,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    let session_directory = session_directory.as_ref();
    let socket = PathBuf::from("<abstract-control-socket>");
    let mut request_frame = serde_json::to_vec(request).map_err(|source| {
        return Error::InvalidJson {
            path: socket.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if u64::try_from(request_frame.len().saturating_add(1)).unwrap_or(u64::MAX)
        > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "request must be no larger than 1 MiB".to_owned(),
        });
    }
    request_frame.push(b'\n');
    let mut stream = connect_session_control(session_directory)?;
    let timeout = control_response_timeout(&request.command, Duration::from_secs(2));
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| return stream.set_write_timeout(Some(timeout)))
        .map_err(|source| {
            return control_transport_error(
                session_directory,
                "configure deadlines",
                timeout,
                false,
                source,
            );
        })?;
    stream.write_all(&request_frame).map_err(|source| {
        return control_transport_error(session_directory, "write request", timeout, true, source);
    })?;
    stream.shutdown(Shutdown::Write).map_err(|source| {
        return control_transport_error(session_directory, "finish request", timeout, true, source);
    })?;

    let mut encoded = Vec::new();
    BufReader::new(stream)
        .take(MAX_CONTROL_MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut encoded)
        .map_err(|source| {
            return control_transport_error(
                session_directory,
                "read response",
                timeout,
                true,
                source,
            );
        })?;
    validate_frame(&encoded)?;
    let response: ControlResponse = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidJson {
            path: socket,
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if response.version != CONTROL_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err(Error::InvalidControlProtocol {
            reason: "response version or request identifier does not match".to_owned(),
        });
    }
    return Ok(response);
}

pub(crate) fn control_response_timeout(command: &ControlCommand, wait_slack: Duration) -> Duration {
    return match command {
        ControlCommand::WaitScreen { timeout_ns, .. }
        | ControlCommand::CaptureStable { timeout_ns, .. } => {
            Duration::from_nanos(*timeout_ns).saturating_add(wait_slack)
        }
        ControlCommand::InputSequence { atoms, pace_ns, .. } => {
            input_sequence_response_timeout(atoms, *pace_ns)
        }
        ControlCommand::Act {
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
        _ => DEFAULT_CONTROL_TIMEOUT,
    };
}

fn input_sequence_response_timeout(atoms: &[InputSequenceAtom], pace_ns: u64) -> Duration {
    let gaps = u64::try_from(atoms.len().saturating_sub(1)).unwrap_or(u64::MAX);
    let pacing = Duration::from_nanos(pace_ns.saturating_mul(gaps)).min(MAX_INPUT_SEQUENCE_PACING);
    return DEFAULT_CONTROL_TIMEOUT.saturating_add(pacing);
}

fn control_lifecycle_hint(session: &Path) -> String {
    if session.join("completion.json").is_file() {
        return "completed".to_owned();
    }
    if !session.join("session.json").is_file() {
        return "not_a_session".to_owned();
    }
    return "control_endpoint_unreachable_or_recorder_starting".to_owned();
}

fn control_transport_error(
    session: &Path,
    phase: &'static str,
    timeout: Duration,
    request_dispatched: bool,
    source: IoError,
) -> Error {
    if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        return Error::ControlTimeout {
            session: session.to_path_buf(),
            phase,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            request_dispatched,
            source,
        };
    }
    return Error::ControlTransport {
        session: session.to_path_buf(),
        phase,
        request_dispatched,
        source,
    };
}

pub(crate) fn validate_frame(encoded: &[u8]) -> Result<()> {
    if encoded.is_empty()
        || encoded.last() != Some(&b'\n')
        || u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "message must be one newline-terminated JSON object no larger than 1 MiB"
                .to_owned(),
        });
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn rejects_unterminated_and_oversized_frames() {
        assert!(validate_frame(br#"{"version":1}"#).is_err());
        let mut oversized = vec![b'x'; usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap()];
        oversized.push(b'\n');
        assert!(validate_frame(&oversized).is_err());
    }

    #[test]
    fn request_schema_rejects_unknown_fields() {
        let encoded = format!(
            r#"{{"version":1,"request_id":"{}","command":"ping","extra":true}}"#,
            Uuid::now_v7()
        );
        assert!(serde_json::from_str::<ControlRequest>(&encoded).is_err());
    }

    #[test]
    fn missing_live_actor_has_a_typed_lifecycle_error() {
        let temporary = TempDir::new().expect("temporary directory");
        let error = request(temporary.path(), ControlCommand::Ping).expect_err("missing actor");
        assert!(matches!(
            error,
            Error::ControlUnavailable { lifecycle, .. } if lifecycle == "not_a_session"
        ));
    }

    #[test]
    fn paced_sequences_extend_both_sides_of_the_control_deadline() {
        let atoms = vec![
            InputSequenceAtom::Bytes {
                operation_id: Uuid::now_v7(),
                data: ByteString::new(vec![b'a']),
                input_kind: InputKind::Text,
                caller_intent: None,
            },
            InputSequenceAtom::Bytes {
                operation_id: Uuid::now_v7(),
                data: ByteString::new(vec![b'b']),
                input_kind: InputKind::Text,
                caller_intent: None,
            },
        ];
        let command = ControlCommand::InputSequence {
            atoms,
            pace_ns: 11_000_000_000,
            include_atom_outcomes: false,
        };

        assert_eq!(
            control_response_timeout(&command, Duration::from_secs(2)),
            Duration::from_secs(21)
        );
    }
}
