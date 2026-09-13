use crate::cli::{
    analysis::args::ShowFormat,
    parsers::{
        parse_bounded_screen_regex, parse_pace_duration, parse_stability_duration,
        parse_state_hash, parse_time_offset_ns, parse_wait_deadline_duration,
    },
};
use clap::{ArgGroup, Args, ValueEnum};
use std::{path::PathBuf, time::Duration};
use termtrace_model::{protocol::control::ScreenMode, terminal::state::CursorStyle};

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("predicate")
        .required(true)
        .multiple(false)
        .args([
            "screen",
            "output",
            "idle",
            "screen_absent",
            "screen_regex",
            "row_contains",
            "region_contains",
            "cursor_at",
            "cursor_visible",
            "cursor_shape",
            "mode",
            "mode_disabled",
            "state_changed",
            "state_unchanged"
        ])
))]
// Require the group so another predicate's conflict cannot waive the idle requirement.
#[command(group(ArgGroup::new("idle_selection").args(["idle"])))]
#[command(override_usage = "termtrace wait [OPTIONS] <SESSION>")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Screen, cursor, and incomplete predicates are independent wait flags"
)]
pub(in crate::cli) struct WaitArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: PathBuf,
    /// Exact UTF-8 text required in the latest presented viewport.
    #[arg(long)]
    pub(in crate::cli) screen: Option<String>,
    /// Exact UTF-8 text to find in canonical output history.
    #[arg(long)]
    pub(in crate::cli) output: Option<String>,
    /// Atomically return the exact presented snapshot after it remains unchanged for this duration.
    #[arg(
        long,
        value_parser = parse_stability_duration,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "100ms"
    )]
    pub(in crate::cli) idle: Option<Duration>,
    /// Exact UTF-8 text required to be absent from the presented viewport.
    #[arg(long)]
    pub(in crate::cli) screen_absent: Option<String>,
    /// Bounded linear-time regular expression required in the viewport.
    #[arg(long, value_parser = parse_bounded_screen_regex)]
    pub(in crate::cli) screen_regex: Option<String>,
    /// Required row substring as `ROW:TEXT`, with a zero-based row.
    #[arg(long)]
    pub(in crate::cli) row_contains: Option<String>,
    /// Required region substring as `LEFT,TOP,RIGHT,BOTTOM:TEXT` with half-open bounds.
    #[arg(long)]
    pub(in crate::cli) region_contains: Option<String>,
    /// Require the cursor at zero-based `COLUMN,ROW`.
    #[arg(long)]
    pub(in crate::cli) cursor_at: Option<String>,
    /// Require visible (`true`) or hidden (`false`) cursor state.
    #[arg(long)]
    pub(in crate::cli) cursor_visible: Option<bool>,
    /// Require one cursor shape.
    #[arg(long, value_enum)]
    pub(in crate::cli) cursor_shape: Option<CursorShapeArg>,
    /// Require one stable terminal mode to be enabled.
    #[arg(long, value_enum)]
    pub(in crate::cli) mode: Option<ScreenModeArg>,
    /// Require one stable terminal mode to be disabled.
    #[arg(long, value_enum)]
    pub(in crate::cli) mode_disabled: Option<ScreenModeArg>,
    /// Require the presented state to differ from this BLAKE3 hash.
    #[arg(long)]
    pub(in crate::cli) state_changed: Option<String>,
    /// Require the presented state to equal this BLAKE3 hash.
    #[arg(long)]
    pub(in crate::cli) state_unchanged: Option<String>,
    /// Require an unchanged matching presented state for this duration.
    #[arg(
        long,
        value_parser = parse_stability_duration,
        default_value = "0ms",
        conflicts_with = "idle"
    )]
    pub(in crate::cli) stable_for: Duration,
    /// Maximum wait duration.
    #[arg(long, value_parser = parse_wait_deadline_duration, default_value = "30s")]
    pub(in crate::cli) timeout: Duration,
    /// Return typed completed/output-closed/deadline snapshots instead of rejecting them.
    #[arg(long, requires = "idle_selection")]
    pub(in crate::cli) allow_incomplete: bool,
    /// Require the stable interval to complete while the recorder is still live.
    #[arg(long, requires = "idle_selection", conflicts_with = "allow_incomplete")]
    pub(in crate::cli) require_live_stability: bool,
    /// Include the compressed complete semantic state in the response.
    #[arg(long, requires = "idle_selection")]
    pub(in crate::cli) full_state: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ScreenEncoding {
    /// Complete screen text including trailing blank cells and whitespace.
    Exact,
    /// Plain trimmed text lines suitable for human inspection.
    Text,
    /// Compact non-empty row text indexed by row numbers.
    Sparse,
    /// Visual text diff against a prior presented state.
    Diff,
    /// Omit screen text from the response payload.
    None,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("screen_state_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
#[expect(clippy::struct_excessive_bools, reason = "Independent CLI flags")]
pub(in crate::cli) struct ScreenArgs {
    /// Live session name or canonical completed session path.
    pub(in crate::cli) session: PathBuf,
    /// Screen representation included in the compact response.
    #[arg(long, value_enum, default_value_t = ScreenEncoding::Text)]
    pub(in crate::cli) screen: ScreenEncoding,
    /// Prior presented-state hash; returns `unchanged` when it is still current.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) since: Option<String>,
    /// Exact sequential state identifier.
    #[arg(long)]
    pub(in crate::cli) state: Option<u64>,
    /// Exact presented state BLAKE3 hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) state_hash: Option<String>,
    /// Sequential canonical event sequence.
    #[arg(long)]
    pub(in crate::cli) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(in crate::cli) at: Option<u64>,
    /// State at the given marker label.
    #[arg(long)]
    pub(in crate::cli) at_marker: Option<String>,
    /// Output representation format.
    #[arg(long, value_enum)]
    pub(in crate::cli) format: Option<ShowFormat>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(in crate::cli) include_hidden: bool,
    /// Explicitly observe an integrity-checked incomplete event prefix offline.
    #[arg(long)]
    pub(in crate::cli) recover_prefix: bool,
    /// Require an unchanged live presented state for this duration.
    #[arg(long, value_parser = parse_stability_duration, default_value = "0ms")]
    pub(in crate::cli) stable_for: Duration,
    /// Maximum live stability wait.
    #[arg(long, value_parser = parse_wait_deadline_duration, default_value = "30s")]
    pub(in crate::cli) timeout: Duration,
    /// Write a concise provenance header to standard error.
    #[arg(long)]
    pub(in crate::cli) metadata: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("send_postcondition")
        .multiple(false)
        .args(["until_idle", "until_screen", "until_screen_regex"])
))]
pub(in crate::cli) struct SendArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: PathBuf,
    /// Named keys, text:VALUE, paste:VALUE, raw:VALUE, or action atoms.
    #[arg(index = 2, value_name = "INPUT", num_args = 0..)]
    pub(in crate::cli) atoms: Vec<String>,
    /// Read raw bytes or atoms from standard input.
    #[arg(long)]
    pub(in crate::cli) stdin: bool,
    /// UTF-8 text to write directly to the child PTY.
    #[arg(long)]
    pub(in crate::cli) text: Option<String>,
    /// File whose exact bytes are written directly to the child PTY.
    #[arg(long)]
    pub(in crate::cli) file: Option<PathBuf>,
    /// Terminal resize as WxH or COLSxROWS.
    #[arg(long)]
    pub(in crate::cli) resize: Option<String>,
    /// Signal to deliver to child process (e.g. INT, TERM, HUP).
    #[arg(long)]
    pub(in crate::cli) signal: Option<String>,
    /// Marker label to append to session timeline.
    #[arg(long)]
    pub(in crate::cli) mark: Option<String>,
    /// Wait until the presented screen contains this exact text.
    #[arg(long)]
    pub(in crate::cli) until_screen: Option<String>,
    /// Wait until the presented screen matches this bounded regular expression.
    #[arg(long, value_parser = parse_bounded_screen_regex)]
    pub(in crate::cli) until_screen_regex: Option<String>,
    /// Wait until the resulting presented screen remains unchanged for this duration.
    #[arg(
        long,
        value_parser = parse_stability_duration,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "100ms"
    )]
    pub(in crate::cli) until_idle: Option<Duration>,
    /// Maximum postcondition wait duration.
    #[arg(long, value_parser = parse_wait_deadline_duration, default_value = "30s")]
    pub(in crate::cli) timeout: Duration,
    /// Delay between atoms; paced text is split by Unicode scalar.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(in crate::cli) pace: Duration,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    pub(in crate::cli) intent: Option<String>,
    /// Apply input only if the latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) if_state: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    /// Include per-atom operation and canonical event identifiers.
    #[arg(long)]
    pub(in crate::cli) verbose: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum CursorShapeArg {
    /// Vertical bar cursor.
    Bar,
    /// Full cell block cursor.
    Block,
    /// Horizontal underline cursor.
    Underline,
    /// Outlined hollow box cursor.
    HollowBlock,
}

impl From<CursorShapeArg> for CursorStyle {
    fn from(value: CursorShapeArg) -> Self {
        return match value {
            CursorShapeArg::Bar => Self::Bar,
            CursorShapeArg::Block => Self::Block,
            CursorShapeArg::Underline => Self::Underline,
            CursorShapeArg::HollowBlock => Self::HollowBlock,
        };
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum ScreenModeArg {
    /// Terminal alternate screen buffer mode (DEC 1049).
    AlternateScreen,
    /// Bracketed paste mode (DEC 2004).
    BracketedPaste,
    /// Automatic terminal line wraparound mode (DEC 7).
    Wraparound,
    /// Synchronized output atomic batch rendering mode (DEC 2026).
    SynchronizedOutput,
}

impl From<ScreenModeArg> for ScreenMode {
    fn from(value: ScreenModeArg) -> Self {
        return match value {
            ScreenModeArg::AlternateScreen => Self::AlternateScreen,
            ScreenModeArg::BracketedPaste => Self::BracketedPaste,
            ScreenModeArg::Wraparound => Self::Wraparound,
            ScreenModeArg::SynchronizedOutput => Self::SynchronizedOutput,
        };
    }
}

#[derive(Debug, Args)]
pub(in crate::cli) struct SessionArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct StatusArgs {
    /// Session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: Option<PathBuf>,
    /// Wait for durable completion instead of returning live counters.
    #[arg(long)]
    pub(in crate::cli) wait_complete: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
    /// Run session garbage collection.
    #[arg(long)]
    pub(in crate::cli) gc: bool,
    /// Delete completed sessions older than this many days during gc.
    #[arg(long)]
    pub(in crate::cli) older_than_days: Option<u64>,
    /// Retain at least this many completed sessions during gc.
    #[arg(long)]
    pub(in crate::cli) keep_latest: Option<u64>,
    /// Remove alias name from registry.
    #[arg(long)]
    pub(in crate::cli) rm: Option<String>,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct StopArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Wait for and validate durable completion after stop is acknowledged.
    #[arg(long)]
    pub(in crate::cli) wait: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}
