use crate::cli::{
    control::args::SessionArgs,
    integrity::args::{RecoverArgs, VerifyArgs},
    parsers::{parse_diagnostic_check, parse_event_type, parse_page_limit, parse_state_hash},
};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use std::path::PathBuf;
use termtrace_engine::timeline::parse_time_offset_ns;
use termtrace_media::helper_protocol;
use termtrace_model::terminal::analysis::DiagnosticCheck;

#[derive(Debug, Args)]
pub(in crate::cli) struct AnalyzeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: Option<PathBuf>,
    /// Emit one stable structured report as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    #[command(subcommand)]
    pub(in crate::cli) command: Option<AnalyzeSubcommand>,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum AnalyzeSubcommand {
    /// Summarize the most recent canonical input action.
    #[command(name = "last-action")]
    LastAction(InspectFacadeArgs),
    /// Replay canonical events and build semantic state artifacts.
    Index(IndexArgs),
    /// Reconstruct one semantic state as JSON or text.
    Inspect(InspectArgs),
    /// List canonical operations and semantic state transitions in order.
    Timeline(TimelineArgs),
    /// Compare two canonical semantic state points.
    Diff(DiffArgs),
    /// Explain exact VT and semantic evidence around one state point.
    Explain(ExplainArgs),
    /// Run conservative evidence-backed rendering diagnostics.
    Diagnose(DiagnoseArgs),
    /// Navigate validated evidence in a read-only semantic terminal UI.
    Review(ReviewArgs),
    /// Write exact canonical child PTY output bytes.
    Output(OutputArgs),
    /// Stream filtered canonical event projections as JSONL.
    Events(EventsArgs),
    /// List canonical marker positions and nominal source frames.
    Markers(MarkersArgs),
    /// Inspect or clear bounded disposable replay checkpoints.
    Cache(CacheArgs),
    /// Verify a completed session's structure and evidence bindings offline.
    Verify(VerifyArgs),
    /// Copy the longest integrity-checked event prefix without modifying source.
    Recover(RecoverArgs),
}

#[derive(Debug, Args)]
pub(in crate::cli) struct InspectFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Minimum changed-cell percentage for a broad repaint warning.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) broad_change_percent: u8,
    /// Emit one stable structured report as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct DiagnoseFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Minimum changed-cell percentage for a broad repaint finding.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) broad_change_percent: u8,
    /// Emit one stable structured report as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum InspectFormat {
    /// Formatted JSON object with complete cell and style records.
    Json,
    /// Single-line compact JSON representation.
    JsonCompact,
    /// Plain text lines without ANSI formatting.
    Text,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "monotonic_ns"])
))]
pub(in crate::cli) struct InspectArgs {
    /// Source session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(in crate::cli) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(in crate::cli) event_seq: Option<u64>,
    /// Latest state at or before this monotonic nanosecond timestamp.
    #[arg(long)]
    pub(in crate::cli) monotonic_ns: Option<u64>,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = InspectFormat::Json)]
    pub(in crate::cli) format: InspectFormat,
    /// Restrict output to one zero-based row; JSON includes complete cell data.
    #[arg(long)]
    pub(in crate::cli) row: Option<u16>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct CacheArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum CacheCommand {
    /// Report retained compressed checkpoint count and byte limits.
    Status(SessionArgs),
    /// Delete disposable checkpoints while preserving canonical evidence.
    Prune(SessionArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ShowFormat {
    /// Plain text representation of the viewport.
    Text,
    /// Complete JSON representation with cell metadata.
    Json,
    /// Compact JSON representation without individual cell details.
    JsonCompact,
    /// Low-level frame JSON matching internal presentation models.
    FrameJson,
    /// Raw VT escape sequences reconstructing the screen state.
    Vt,
    /// Self-contained HTML document rendering the viewport.
    Html,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ShowRenderer {
    /// Pure semantic cell grid without native Ghostty rasterization.
    Semantic,
    /// Pixel-exact rendering using the native Ghostty VT engine.
    Native,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TimelineArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Include synchronized-output states that were not presentation eligible.
    #[arg(long)]
    pub(in crate::cli) all_states: bool,
    /// Include lossless framed VT controls and printable runs.
    #[arg(long)]
    pub(in crate::cli) controls: bool,
    /// Emit one stable JSON object per line.
    #[arg(long)]
    pub(in crate::cli) jsonl: bool,
    #[command(flatten)]
    pub(in crate::cli) page: PageArgs,
    #[command(flatten)]
    pub(in crate::cli) range: TimeRangeArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum DiffFormat {
    /// Unified plain text terminal diff.
    Text,
    /// Formatted JSON object containing detailed cell mutations.
    Json,
    /// Single-line compact JSON diff representation.
    JsonCompact,
    /// Scalable Vector Graphics visual diff visualization.
    Svg,
    /// Portable Network Graphics raster diff image.
    Png,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("diff_from")
        .required(true)
        .multiple(false)
        .args(["from_state", "from_state_hash", "from_event_seq", "from_at", "from_marker"])
))]
#[command(group(
    ArgGroup::new("diff_to")
        .required(true)
        .multiple(false)
        .args(["to_state", "to_state_hash", "to_event_seq", "to_at", "to_marker"])
))]
#[command(
    override_usage = "termtrace analyze diff [OPTIONS] <SESSION> <FROM_SELECTION> <TO_SELECTION>"
)]
pub(in crate::cli) struct DiffArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Earlier exact semantic state identifier.
    #[arg(long, help_heading = "From selection")]
    pub(in crate::cli) from_state: Option<u64>,
    /// Earlier exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "From selection")]
    pub(in crate::cli) from_state_hash: Option<String>,
    /// Earlier state at or before this event sequence.
    #[arg(long, help_heading = "From selection")]
    pub(in crate::cli) from_event_seq: Option<u64>,
    /// Earlier state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "From selection")]
    pub(in crate::cli) from_at: Option<u64>,
    /// Earlier state at a unique marker.
    #[arg(long, help_heading = "From selection")]
    pub(in crate::cli) from_marker: Option<String>,
    /// Later exact semantic state identifier.
    #[arg(long, help_heading = "To selection")]
    pub(in crate::cli) to_state: Option<u64>,
    /// Later exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "To selection")]
    pub(in crate::cli) to_state_hash: Option<String>,
    /// Later state at or before this event sequence.
    #[arg(long, help_heading = "To selection")]
    pub(in crate::cli) to_event_seq: Option<u64>,
    /// Later state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "To selection")]
    pub(in crate::cli) to_at: Option<u64>,
    /// Later state at a unique marker.
    #[arg(long, help_heading = "To selection")]
    pub(in crate::cli) to_marker: Option<String>,
    /// Fail when any semantic or presentation value changed.
    #[arg(long)]
    pub(in crate::cli) expect_no_change: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
    pub(in crate::cli) format: DiffFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum ExplainFormat {
    /// Plain text breakdown of VT events and state transitions.
    Text,
    /// Formatted JSON object detailing causal evidence.
    Json,
    /// Single-line compact JSON explanation representation.
    JsonCompact,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("explain_target")
        .required(true)
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "marker"])
))]
#[command(override_usage = "termtrace analyze explain [OPTIONS] <SESSION> <TARGET_SELECTION>")]
pub(in crate::cli) struct ExplainArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long, help_heading = "Target selection")]
    pub(in crate::cli) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "Target selection")]
    pub(in crate::cli) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long, help_heading = "Target selection")]
    pub(in crate::cli) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "Target selection")]
    pub(in crate::cli) at: Option<u64>,
    /// State at a unique marker.
    #[arg(long, help_heading = "Target selection")]
    pub(in crate::cli) marker: Option<String>,
    /// Canonical event radius before and after the target.
    #[arg(long, default_value_t = 8)]
    pub(in crate::cli) context: u64,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ExplainFormat::Text)]
    pub(in crate::cli) format: ExplainFormat,
    #[command(flatten)]
    pub(in crate::cli) page: PageArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct DiagnoseArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Run diagnostics bounded to the most recent canonical input action.
    #[arg(long)]
    pub(in crate::cli) last_action: bool,
    #[arg(long, value_delimiter = ',', value_parser = parse_diagnostic_check)]
    pub(in crate::cli) check: Vec<DiagnosticCheck>,
    /// Minimum changed-cell percentage for a broad repaint.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) broad_change_percent: u8,
    /// Emit one stable structured report.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    /// Exit unsuccessfully when at least one finding is emitted.
    #[arg(long)]
    pub(in crate::cli) fail_on_finding: bool,
    #[command(flatten)]
    pub(in crate::cli) page: PageArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ReviewArgs {
    /// Completed canonical session or registered name.
    pub(in crate::cli) session: Option<PathBuf>,
    /// Start at the first semantic state at or after this unique marker.
    #[arg(long)]
    pub(in crate::cli) around_marker: Option<String>,
    /// Exact semantic viewport substring to find and navigate with n/N.
    #[arg(long)]
    pub(in crate::cli) search: Option<String>,
    /// Export one exact semantic state to a new owner-only directory.
    #[arg(long, requires = "state")]
    pub(in crate::cli) export_state: Option<PathBuf>,
    /// Exact state identifier for `--export-state`.
    #[arg(long, requires = "export_state")]
    pub(in crate::cli) state: Option<u64>,
    /// Print scriptable reviewer commands and safety policy.
    #[arg(long, conflicts_with = "json")]
    pub(in crate::cli) commands: bool,
    /// Print a validated read-only source summary without opening a TUI.
    #[arg(long, conflicts_with = "commands")]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("output_representation")
        .required(true)
        .args(["raw", "strip_controls"])
))]
pub(in crate::cli) struct OutputArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Write exact concatenated child PTY bytes without decoding.
    #[arg(long)]
    pub(in crate::cli) raw: bool,
    /// Emit a named lossy projection retaining printable bytes and line controls.
    #[arg(long)]
    pub(in crate::cli) strip_controls: bool,
    /// New no-replace JSON report path; unavailable while following.
    #[arg(long)]
    pub(in crate::cli) manifest: Option<PathBuf>,
    /// Continue emitting newly durable output until the session completes.
    #[arg(long)]
    pub(in crate::cli) follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    pub(in crate::cli) from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    pub(in crate::cli) to_event_seq: Option<u64>,
    #[command(flatten)]
    pub(in crate::cli) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct EventsArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Comma-separated canonical event variant names.
    #[arg(long = "type", value_delimiter = ',', value_parser = parse_event_type)]
    pub(in crate::cli) event_types: Vec<String>,
    /// Elide exact byte payloads while retaining their lengths and hashes.
    #[arg(long)]
    pub(in crate::cli) metadata_only: bool,
    /// Continue streaming newly durable records until completion.
    #[arg(long)]
    pub(in crate::cli) follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    pub(in crate::cli) from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    pub(in crate::cli) to_event_seq: Option<u64>,
    #[command(flatten)]
    pub(in crate::cli) page: PageArgs,
    #[command(flatten)]
    pub(in crate::cli) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PageArgs {
    /// Maximum matching records returned by default (1-10000).
    #[arg(long, default_value_t = 100, value_parser = parse_page_limit)]
    pub(in crate::cli) limit: usize,
    /// Zero-based matching-record offset for deterministic pagination.
    #[arg(long, default_value_t = 0)]
    pub(in crate::cli) offset: usize,
    /// Return every matching record; explicit potentially-large output.
    #[arg(long)]
    pub(in crate::cli) all: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(in crate::cli) struct TimeRangeArgs {
    /// Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`.
    #[arg(long = "from", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "from_marker")]
    pub(in crate::cli) start_monotonic_ns: Option<u64>,
    /// Inclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "start_monotonic_ns")]
    pub(in crate::cli) from_marker: Option<String>,
    /// Exclusive source offset, in the same format as `--from`.
    #[arg(long = "to", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "to_marker")]
    pub(in crate::cli) end_monotonic_ns: Option<u64>,
    /// Exclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "end_monotonic_ns")]
    pub(in crate::cli) to_marker: Option<String>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct IndexArgs {
    /// Source session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// Index a hash-addressed prefix when the recording is still active.
    #[arg(long)]
    pub(in crate::cli) live_snapshot: bool,
    /// Retain only states eligible for native terminal presentation.
    #[arg(long)]
    pub(in crate::cli) presented_only: bool,
    #[command(flatten)]
    pub(in crate::cli) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
#[command(override_usage = playback_usage())]
pub(in crate::cli) struct PlaybackArgs {
    /// Source session directory.
    #[arg(long = helper_protocol::playback::SESSION)]
    pub(in crate::cli) session: PathBuf,
    /// Playback clock handshake path.
    #[arg(long = helper_protocol::playback::TIMING_FILE)]
    pub(in crate::cli) timing_file: PathBuf,
    /// Structured failure handshake path.
    #[arg(long = helper_protocol::playback::ERROR_FILE)]
    pub(in crate::cli) error_file: PathBuf,
    /// Playback-terminal geometry handshake path.
    #[arg(long = helper_protocol::playback::GEOMETRY_FILE)]
    pub(in crate::cli) geometry_file: PathBuf,
    /// Final-frame readiness path.
    #[arg(long = helper_protocol::playback::READY_FILE)]
    pub(in crate::cli) ready_file: PathBuf,
    /// Delay before event time zero.
    #[arg(long = helper_protocol::playback::START_DELAY_MS)]
    pub(in crate::cli) start_delay_ms: u64,
    /// Final-frame hold duration.
    #[arg(long = helper_protocol::playback::FINAL_HOLD_MS)]
    pub(in crate::cli) final_hold_ms: u64,
    /// Inclusive source offset used by ranged native capture.
    #[arg(long = helper_protocol::playback::FROM_NS)]
    pub(in crate::cli) from_ns: Option<u64>,
    /// Exclusive source offset used by ranged native capture.
    #[arg(long = helper_protocol::playback::TO_NS)]
    pub(in crate::cli) to_ns: Option<u64>,
    /// Exact endpoint event used by native single-state capture.
    #[arg(long = helper_protocol::playback::ENDPOINT_EVENT_SEQ)]
    pub(in crate::cli) endpoint_event_seq: Option<u64>,
    /// Inclusive byte offset within the endpoint output event.
    #[arg(long = helper_protocol::playback::ENDPOINT_EVENT_BYTE_OFFSET)]
    pub(in crate::cli) endpoint_event_byte_offset: Option<u64>,
    /// Disable source-time sleeps while replaying an exact endpoint.
    #[arg(long = helper_protocol::playback::INSTANT)]
    pub(in crate::cli) instant: bool,
}

/// Playback usage wrapped to stay within narrow help widths.
fn playback_usage() -> String {
    use helper_protocol::playback::{
        ERROR_FILE, GEOMETRY_FILE, READY_FILE, SESSION, SUBCOMMAND, TIMING_FILE,
    };

    return format!(
        "termtrace {SUBCOMMAND} [OPTIONS] --{SESSION} <SESSION>\n       --{TIMING_FILE} <TIMING_FILE> --{ERROR_FILE} <ERROR_FILE>\n       --{GEOMETRY_FILE} <GEOMETRY_FILE> --{READY_FILE} <READY_FILE>"
    );
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PlaybackWorkerArgs {
    /// Canonical source session.
    #[arg(long = helper_protocol::playback_worker::SESSION)]
    pub(in crate::cli) session: PathBuf,
    /// Private parent protocol socket.
    #[arg(long = helper_protocol::playback_worker::SOCKET)]
    pub(in crate::cli) socket: PathBuf,
    /// Initial measured terminal geometry.
    #[arg(long = helper_protocol::playback_worker::GEOMETRY_FILE)]
    pub(in crate::cli) geometry_file: PathBuf,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct MarkersArgs {
    /// Completed canonical session.
    pub(in crate::cli) session: PathBuf,
    /// Nominal frame rate used only for source-time mapping.
    #[arg(long, default_value_t = 24)]
    pub(in crate::cli) fps: u16,
}
