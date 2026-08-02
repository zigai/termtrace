use super::*;

#[derive(Debug, Args)]
pub(super) struct ControlArgs {
    #[command(subcommand)]
    pub(super) command: ControlSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ControlSubcommand {
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
            "output",
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
#[command(override_usage = "termtrace wait [OPTIONS] <SESSION> <SCREEN|PREDICATE_OPTION>")]
pub(super) struct WaitArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(super) session: PathBuf,
    /// Exact UTF-8 text required in the latest presented viewport.
    #[arg(index = 2)]
    pub(super) screen: Option<String>,
    /// Exact UTF-8 text to find in canonical output history.
    #[arg(long)]
    pub(super) output: Option<String>,
    /// Exact UTF-8 text required to be absent from the presented viewport.
    #[arg(long)]
    pub(super) screen_absent: Option<String>,
    /// Bounded linear-time regular expression required in the viewport.
    #[arg(long)]
    pub(super) screen_regex: Option<String>,
    /// Required row substring as `ROW:TEXT`, with a zero-based row.
    #[arg(long)]
    pub(super) row_contains: Option<String>,
    /// Required region substring as `LEFT,TOP,RIGHT,BOTTOM:TEXT` with half-open bounds.
    #[arg(long)]
    pub(super) region_contains: Option<String>,
    /// Require the cursor at zero-based `COLUMN,ROW`.
    #[arg(long)]
    pub(super) cursor_at: Option<String>,
    /// Require visible (`true`) or hidden (`false`) cursor state.
    #[arg(long)]
    pub(super) cursor_visible: Option<bool>,
    /// Require one cursor shape.
    #[arg(long, value_enum)]
    pub(super) cursor_shape: Option<CursorShapeArg>,
    /// Require one stable terminal mode to be enabled.
    #[arg(long, value_enum)]
    pub(super) mode: Option<ScreenModeArg>,
    /// Require one stable terminal mode to be disabled.
    #[arg(long, value_enum)]
    pub(super) mode_disabled: Option<ScreenModeArg>,
    /// Require the presented state to differ from this BLAKE3 hash.
    #[arg(long)]
    pub(super) state_changed: Option<String>,
    /// Require the presented state to equal this BLAKE3 hash.
    #[arg(long)]
    pub(super) state_unchanged: Option<String>,
    /// Require an unchanged matching presented state for this duration.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(super) stable_for: Duration,
    /// Maximum wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(super) timeout_ms: u64,
}

#[derive(Debug, Args)]
pub(super) struct WaitIdleArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Required unchanged presented-state interval.
    #[arg(long, value_parser = parse_pace_duration, default_value = "100ms")]
    pub(super) stable_for: Duration,
    /// Maximum server-side wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(super) timeout_ms: u64,
    /// Return typed completed/output-closed/deadline snapshots instead of rejecting them.
    #[arg(long)]
    pub(super) allow_incomplete: bool,
    /// Include the compressed complete semantic state in the response.
    #[arg(long)]
    pub(super) full_state: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum CursorShapeArg {
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
pub(super) enum ScreenModeArg {
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
pub(super) struct SessionArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
}

#[derive(Debug, Args)]
pub(super) struct StatusArgs {
    /// Session name or canonical path.
    pub(super) session: PathBuf,
    /// Wait for durable completion instead of returning live counters.
    #[arg(long)]
    pub(super) wait_complete: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(super) timeout_ms: u64,
}

#[derive(Debug, Args)]
pub(super) struct StopArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Wait for and validate durable completion after stop is acknowledged.
    #[arg(long)]
    pub(super) wait: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(super) timeout_ms: u64,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
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
pub(super) struct SendArgs {
    /// Live session name or canonical path.
    #[arg(index = 1)]
    pub(super) session: PathBuf,
    /// Named keys, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH atoms.
    #[arg(index = 2, value_name = "ATOM", num_args = 1..)]
    pub(super) atoms: Vec<String>,
    /// Read one exact raw burst from standard input.
    #[arg(long)]
    pub(super) stdin: bool,
    /// Delay between atoms; paced text is split by Unicode scalar.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(super) pace: Duration,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    pub(super) intent: Option<String>,
    /// Apply every input atom only if the latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
    /// Include per-atom operation and canonical event identifiers in the response.
    #[arg(long)]
    pub(super) verbose: bool,
}

#[derive(Debug, Args)]
pub(super) struct ResizeArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Terminal columns.
    #[arg(long)]
    pub(super) cols: u16,
    /// Terminal rows.
    #[arg(long)]
    pub(super) rows: u16,
    /// Total pixel width; defaults to columns multiplied by 12.
    #[arg(long)]
    pub(super) pixel_width: Option<u16>,
    /// Total pixel height; defaults to rows multiplied by 24.
    #[arg(long)]
    pub(super) pixel_height: Option<u16>,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum FocusDirection {
    In,
    Out,
}

#[derive(Debug, Args)]
pub(super) struct FocusArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Focus transition to report.
    #[arg(value_enum)]
    pub(super) direction: FocusDirection,
    /// Reject when reporting is disabled instead of returning a semantic no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum CliPointerButton {
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
pub(super) struct PointerArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    #[command(subcommand)]
    pub(super) operation: PointerCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum PointerCommand {
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
pub(super) struct PointerClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(super) cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(super) button: CliPointerButton,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    pub(super) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(super) struct PointerMultiClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(super) cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(super) button: CliPointerButton,
    /// Number of complete clicks.
    #[arg(long, value_parser = clap::value_parser!(u8).range(2..=4))]
    pub(super) count: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    pub(super) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(super) struct PointerDragArgs {
    /// Zero-based COLUMN,ROW press cell.
    #[arg(long, value_parser = parse_cell)]
    pub(super) from: (u16, u16),
    /// Zero-based COLUMN,ROW release cell.
    #[arg(long, value_parser = parse_cell)]
    pub(super) to: (u16, u16),
    /// Button held throughout the drag.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    pub(super) button: CliPointerButton,
    /// Number of motion samples, including the destination.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(super) steps: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    pub(super) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(super) struct PointerScrollArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    pub(super) cell: (u16, u16),
    /// Signed lines: negative is up and positive is down.
    #[arg(long, allow_negative_numbers = true)]
    pub(super) lines: i16,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    pub(super) postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
pub(super) struct PointerPostconditionArgs {
    /// After applying input, wait until the presented screen contains this text.
    #[arg(long, conflicts_with = "wait_screen_absent")]
    pub(super) wait_screen: Option<String>,
    /// After applying input, wait until the presented screen excludes this text.
    #[arg(long, conflicts_with = "wait_screen")]
    pub(super) wait_screen_absent: Option<String>,
    /// Require one unchanged matching presented state for this duration.
    #[arg(long, default_value = "0ms", value_parser = parse_pace_duration)]
    pub(super) stable_for: Duration,
    /// Bounded postcondition deadline.
    #[arg(long, default_value_t = 10_000)]
    pub(super) timeout_ms: u64,
}

#[derive(Debug, Args)]
pub(super) struct ActivateArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Exact visible row-contained text to resolve.
    #[arg(long)]
    pub(super) text: String,
    /// Require exactly one visible match.
    #[arg(long)]
    pub(super) unique: bool,
    /// Reject when mouse reporting suppresses output instead of returning a no-op.
    #[arg(long)]
    pub(super) strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum CliSignal {
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
pub(super) struct SignalArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Allow-listed signal.
    #[arg(long, value_enum)]
    pub(super) signal: CliSignal,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct MarkArgs {
    /// Live session name or canonical path.
    pub(super) session: PathBuf,
    /// Marker label.
    #[arg(long)]
    pub(super) label: String,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    pub(super) expected_presented_state_hash: Option<String>,
}
