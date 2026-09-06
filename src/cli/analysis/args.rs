use std::{path::PathBuf, time::Duration};

use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use termtrace_model::terminal::analysis::DiagnosticCheck;

use crate::cli::{
    control::args::SessionArgs,
    parsers::{
        parse_diagnostic_check, parse_event_type, parse_page_limit, parse_stability_duration,
        parse_state_hash,
    },
};
use termtrace_engine::timeline::parse_time_offset_ns;

#[derive(Debug, Args)]
pub(in crate::cli) struct AnalyzeArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: AnalyzeSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum AnalyzeSubcommand {
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
}

#[derive(Debug, Args)]
pub(in crate::cli) struct InspectFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Minimum changed-cell percentage for a broad repaint warning.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) broad_change_percent: u8,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct DiagnoseFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Minimum changed-cell percentage for a broad repaint finding.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(in crate::cli) broad_change_percent: u8,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum InspectFormat {
    Json,
    JsonCompact,
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
    Text,
    Json,
    JsonCompact,
    FrameJson,
    Vt,
    Html,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ShowRenderer {
    Semantic,
    Native,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("show_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
pub(in crate::cli) struct ShowArgs {
    /// Live session name or canonical completed session path.
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
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(in crate::cli) at: Option<u64>,
    /// State at a unique marker label.
    #[arg(long)]
    pub(in crate::cli) at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(in crate::cli) include_hidden: bool,
    /// Require an unchanged live presented state for this duration.
    #[arg(long, value_parser = parse_stability_duration, default_value = "0ms")]
    pub(in crate::cli) stable_for: Duration,
    /// Maximum live stability wait.
    #[arg(long, value_parser = crate::cli::parsers::parse_wait_deadline_duration, default_value = "30s")]
    pub(in crate::cli) timeout: Duration,
    /// Write a concise provenance header to standard error.
    #[arg(long)]
    pub(in crate::cli) metadata: bool,
    /// Explicitly observe an integrity-checked incomplete event prefix offline.
    #[arg(long)]
    pub(in crate::cli) recover_prefix: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ShowFormat::Text)]
    pub(in crate::cli) format: ShowFormat,
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
    Text,
    Json,
    JsonCompact,
    Svg,
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
    Text,
    Json,
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
    /// Comma-separated detector groups; omit to run every detector.
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
    /// Maximum matching records returned by default.
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
#[command(
    override_usage = "termtrace playback [OPTIONS] --session <SESSION>\n       --timing-file <TIMING_FILE> --error-file <ERROR_FILE>\n       --geometry-file <GEOMETRY_FILE> --ready-file <READY_FILE>"
)]
pub(in crate::cli) struct PlaybackArgs {
    /// Source session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// Playback clock handshake path.
    #[arg(long)]
    pub(in crate::cli) timing_file: PathBuf,
    /// Structured failure handshake path.
    #[arg(long)]
    pub(in crate::cli) error_file: PathBuf,
    /// Playback-terminal geometry handshake path.
    #[arg(long)]
    pub(in crate::cli) geometry_file: PathBuf,
    /// Final-frame readiness path.
    #[arg(long)]
    pub(in crate::cli) ready_file: PathBuf,
    /// Delay before event time zero.
    #[arg(long)]
    pub(in crate::cli) start_delay_ms: u64,
    /// Final-frame hold duration.
    #[arg(long)]
    pub(in crate::cli) final_hold_ms: u64,
    /// Inclusive source offset used by ranged native capture.
    #[arg(long)]
    pub(in crate::cli) from_ns: Option<u64>,
    /// Exclusive source offset used by ranged native capture.
    #[arg(long)]
    pub(in crate::cli) to_ns: Option<u64>,
    /// Exact endpoint event used by native single-state capture.
    #[arg(long)]
    pub(in crate::cli) endpoint_event_seq: Option<u64>,
    /// Inclusive byte offset within the endpoint output event.
    #[arg(long)]
    pub(in crate::cli) endpoint_event_byte_offset: Option<u64>,
    /// Disable source-time sleeps while replaying an exact endpoint.
    #[arg(long)]
    pub(in crate::cli) instant: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct PlaybackWorkerArgs {
    /// Canonical source session.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// Private parent protocol socket.
    #[arg(long)]
    pub(in crate::cli) socket: PathBuf,
    /// Initial measured terminal geometry.
    #[arg(long)]
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
