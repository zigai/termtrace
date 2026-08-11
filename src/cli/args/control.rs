use super::*;

#[derive(Debug, Args)]
pub(in crate::cli) struct ControlArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: ControlSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum ControlSubcommand {
    /// Verify that a live session control socket is responsive.
    Ping(SessionArgs),
    /// Print the accepted symbolic key and modifier grammar.
    Keys,
    /// Report terminal-window focus when the application opted in.
    Focus(FocusArgs),
    /// Send cell-addressed mouse input using live tracking modes.
    Pointer(PointerArgs),
    /// Resolve visible text and click its center from one actor state.
    Activate(ActivateArgs),
    /// Resize a live child PTY.
    Resize(ResizeArgs),
    /// Signal a live child process group.
    Signal(SignalArgs),
    /// Add a marker to a live session timeline.
    Mark(MarkArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("predicate")
        .required(true)
        .multiple(false)
        .args([
            "screen",
            "screen_compat",
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
#[command(override_usage = "termtrace wait [OPTIONS] <SESSION>")]
pub(in crate::cli) struct WaitArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: PathBuf,
    /// Exact UTF-8 text required in the latest presented viewport.
    #[arg(long)]
    pub(in crate::cli) screen: Option<String>,
    /// Compatibility form for the former positional screen predicate.
    #[arg(index = 2, hide = true)]
    pub(in crate::cli) screen_compat: Option<String>,
    /// Exact UTF-8 text to find in canonical output history.
    #[arg(long)]
    pub(in crate::cli) output: Option<String>,
    /// Wait for an unchanged presented screen, optionally choosing the interval.
    #[arg(
        long,
        value_parser = parse_stability_duration,
        num_args = 0..=1,
        default_missing_value = "100ms"
    )]
    pub(in crate::cli) idle: Option<Duration>,
    /// Exact UTF-8 text required to be absent from the presented viewport.
    #[arg(long)]
    pub(in crate::cli) screen_absent: Option<String>,
    /// Bounded linear-time regular expression required in the viewport.
    #[arg(long)]
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
    #[arg(long, value_parser = parse_stability_duration, default_value = "0ms")]
    pub(in crate::cli) stable_for: Duration,
    /// Maximum wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
    /// Duration-form maximum wait; preferred over the millisecond compatibility option.
    #[arg(long, value_parser = parse_wait_deadline_duration, conflicts_with = "timeout_ms")]
    pub(in crate::cli) timeout: Option<Duration>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct WaitIdleArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Required unchanged presented-state interval.
    #[arg(long, value_parser = parse_stability_duration, default_value = "100ms")]
    pub(in crate::cli) stable_for: Duration,
    /// Maximum server-side wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
    /// Return typed completed/output-closed/deadline snapshots instead of rejecting them.
    #[arg(long)]
    pub(in crate::cli) allow_incomplete: bool,
    /// Require the stable interval to complete while the recorder is still live.
    #[arg(long, conflicts_with = "allow_incomplete")]
    pub(in crate::cli) require_live_stability: bool,
    /// Include the compressed complete semantic state in the response.
    #[arg(long)]
    pub(in crate::cli) full_state: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ScreenEncoding {
    Exact,
    Text,
    Sparse,
    Diff,
    None,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ScreenArgs {
    /// Live session name or canonical completed session path.
    pub(in crate::cli) session: PathBuf,
    /// Screen representation included in the compact response.
    #[arg(long, value_enum, default_value_t = ScreenEncoding::Text)]
    pub(in crate::cli) screen: ScreenEncoding,
    /// Prior presented-state hash; returns `unchanged` when it is still current.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) since: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("act_postcondition")
        .multiple(false)
        .args(["until_idle", "until_screen", "until_screen_regex"])
))]
pub(in crate::cli) struct ActArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: PathBuf,
    /// Named keys, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH atoms.
    #[arg(index = 2, value_name = "INPUT", num_args = 1..)]
    pub(in crate::cli) atoms: Vec<String>,
    /// Delay between atoms; paced text is split by Unicode scalar.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(in crate::cli) pace: Duration,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    pub(in crate::cli) intent: Option<String>,
    /// Apply every input atom only if the latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    /// Wait until the resulting presented screen remains unchanged for this duration.
    #[arg(
        long,
        value_parser = parse_stability_duration,
        num_args = 0..=1,
        default_missing_value = "100ms"
    )]
    pub(in crate::cli) until_idle: Option<Duration>,
    /// Wait until the presented screen contains this exact text.
    #[arg(long)]
    pub(in crate::cli) until_screen: Option<String>,
    /// Wait until the presented screen matches this bounded regular expression.
    #[arg(long)]
    pub(in crate::cli) until_screen_regex: Option<String>,
    /// Maximum postcondition wait duration.
    #[arg(long, value_parser = parse_wait_deadline_duration, default_value = "30s")]
    pub(in crate::cli) timeout: Duration,
    /// Include per-atom operation and canonical event identifiers.
    #[arg(long)]
    pub(in crate::cli) verbose: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum CursorShapeArg {
    Bar,
    Block,
    Underline,
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
    AlternateScreen,
    BracketedPaste,
    Wraparound,
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
    pub(in crate::cli) session: PathBuf,
    /// Wait for durable completion instead of returning live counters.
    #[arg(long)]
    pub(in crate::cli) wait_complete: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
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
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("input")
        .required(true)
        .multiple(false)
        .args(["atoms", "stdin"])
))]
#[command(
    override_usage = "termtrace send [OPTIONS] <SESSION> <ATOM>...\n       termtrace send [OPTIONS] --stdin <SESSION>"
)]
pub(in crate::cli) struct SendArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(in crate::cli) session: PathBuf,
    /// Named keys, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH atoms.
    #[arg(index = 2, value_name = "ATOM", num_args = 1..)]
    pub(in crate::cli) atoms: Vec<String>,
    /// Read one exact raw burst from standard input.
    #[arg(long)]
    pub(in crate::cli) stdin: bool,
    /// Delay between atoms; paced text is split by Unicode scalar.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(in crate::cli) pace: Duration,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    pub(in crate::cli) intent: Option<String>,
    /// Apply every input atom only if the latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    /// Include per-atom operation and canonical event identifiers in the response.
    #[arg(long)]
    pub(in crate::cli) verbose: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ResizeArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Terminal columns.
    #[arg(long)]
    pub(in crate::cli) cols: u16,
    /// Terminal rows.
    #[arg(long)]
    pub(in crate::cli) rows: u16,
    /// Total pixel width; defaults to columns multiplied by 12.
    #[arg(long)]
    pub(in crate::cli) pixel_width: Option<u16>,
    /// Total pixel height; defaults to rows multiplied by 24.
    #[arg(long)]
    pub(in crate::cli) pixel_height: Option<u16>,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum FocusDirection {
    In,
    Out,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct FocusArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Focus transition to report.
    #[arg(value_enum)]
    pub(in crate::cli) direction: FocusDirection,
    /// Reject when reporting is disabled instead of returning a semantic no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum CliPointerButton {
    Left,
    Middle,
    Right,
}

impl From<CliPointerButton> for MouseButton {
    fn from(button: CliPointerButton) -> Self {
        return match button {
            CliPointerButton::Left => Self::Left,
            CliPointerButton::Middle => Self::Middle,
            CliPointerButton::Right => Self::Right,
        };
    }
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    #[command(subcommand)]
    pub(in crate::cli) operation: PointerCommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PointerCommand {
    /// Press and release one button at a zero-based cell.
    Click(PointerClickArgs),
    /// Repeat a complete click two through four times.
    MultiClick(PointerMultiClickArgs),
    /// Press, emit bounded motion samples, and release at a destination cell.
    Drag(PointerDragArgs),
    /// Emit signed vertical wheel ticks at a zero-based cell.
    Scroll(PointerScrollArgs),
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(in crate::cli) cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(in crate::cli) button: CliPointerButton,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    #[command(flatten)]
    pub(in crate::cli) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerMultiClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(in crate::cli) cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(in crate::cli) button: CliPointerButton,
    /// Number of complete clicks.
    #[arg(long, value_parser = clap::value_parser!(u8).range(2..=4))]
    pub(in crate::cli) count: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    #[command(flatten)]
    pub(in crate::cli) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerDragArgs {
    /// Zero-based COLUMN,ROW press cell.
    #[arg(long, value_parser = parse_cell)]
    pub(in crate::cli) from: (u16, u16),
    /// Zero-based COLUMN,ROW release cell.
    #[arg(long, value_parser = parse_cell)]
    pub(in crate::cli) to: (u16, u16),
    /// Button held throughout the drag.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(in crate::cli) button: CliPointerButton,
    /// Number of motion samples, including the destination.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) steps: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    #[command(flatten)]
    pub(in crate::cli) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerScrollArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(in crate::cli) cell: (u16, u16),
    /// Signed lines: negative is up and positive is down.
    #[arg(long, allow_negative_numbers = true)]
    pub(in crate::cli) lines: i16,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
    #[command(flatten)]
    pub(in crate::cli) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PointerPostconditionArgs {
    /// After applying input, wait until the presented screen contains this text.
    #[arg(long, conflicts_with = "wait_screen_absent")]
    pub(in crate::cli) wait_screen: Option<String>,
    /// After applying input, wait until the presented screen excludes this text.
    #[arg(long, conflicts_with = "wait_screen")]
    pub(in crate::cli) wait_screen_absent: Option<String>,
    /// Require one unchanged matching presented state for this duration.
    #[arg(long, default_value = "0ms", value_parser = parse_stability_duration)]
    pub(in crate::cli) stable_for: Duration,
    /// Bounded postcondition deadline.
    #[arg(long, default_value_t = 10_000)]
    pub(in crate::cli) timeout_ms: u64,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ActivateArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Exact visible row-contained text to resolve.
    #[arg(long)]
    pub(in crate::cli) text: String,
    /// Require exactly one visible match.
    #[arg(long)]
    pub(in crate::cli) unique: bool,
    /// Reject when mouse reporting suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(in crate::cli) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum CliSignal {
    Interrupt,
    Terminate,
    Hangup,
    User1,
    User2,
}

impl From<CliSignal> for ProcessSignal {
    fn from(signal: CliSignal) -> Self {
        match signal {
            CliSignal::Interrupt => return Self::Interrupt,
            CliSignal::Terminate => return Self::Terminate,
            CliSignal::Hangup => return Self::Hangup,
            CliSignal::User1 => return Self::User1,
            CliSignal::User2 => return Self::User2,
        }
    }
}

#[derive(Debug, Args)]
pub(in crate::cli) struct SignalArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Allow-listed signal.
    #[arg(long, value_enum)]
    pub(in crate::cli) signal: CliSignal,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct MarkArgs {
    /// Live session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Marker label.
    #[arg(long)]
    pub(in crate::cli) label: String,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(in crate::cli) expected_presented_state_hash: Option<String>,
    /// Caller-owned identifier for idempotent retries.
    #[arg(long)]
    pub(in crate::cli) request_id: Option<uuid::Uuid>,
}
