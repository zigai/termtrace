use super::*;

#[derive(Debug, Args)]
pub(super) struct AnalyzeArgs {
    #[command(subcommand)]
    pub(super) command: AnalyzeSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AnalyzeSubcommand {
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

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum InspectFormat {
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
pub(super) struct InspectArgs {
    /// Source session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(super) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(super) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(super) event_seq: Option<u64>,
    /// Latest state at or before this monotonic nanosecond timestamp.
    #[arg(long)]
    pub(super) monotonic_ns: Option<u64>,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = InspectFormat::Json)]
    pub(super) format: InspectFormat,
    /// Restrict output to one zero-based row; JSON includes complete cell data.
    #[arg(long)]
    pub(super) row: Option<u16>,
}

#[derive(Debug, Args)]
pub(super) struct CacheArgs {
    #[command(subcommand)]
    pub(super) command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum CacheCommand {
    /// Report retained compressed checkpoint count and byte limits.
    Status(SessionArgs),
    /// Delete disposable checkpoints while preserving canonical evidence.
    Prune(SessionArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ShowFormat {
    Text,
    Json,
    JsonCompact,
    FrameJson,
    Vt,
    Html,
    Svg,
    Png,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ShowRenderer {
    Semantic,
    Native,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("show_source")
        .required(true)
        .multiple(false)
        .args(["session", "input", "command"])
))]
#[command(group(
    ArgGroup::new("show_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
pub(super) struct ShowArgs {
    /// Live session name or canonical completed session path.
    pub(super) session: Option<PathBuf>,
    /// ANSI/VT byte stream path, or `-` for standard input.
    #[arg(long)]
    pub(super) input: Option<PathBuf>,
    /// Promote an ANSI input stream into a new canonical import session.
    #[arg(long)]
    pub(super) record_out: Option<PathBuf>,
    /// ANSI stream columns.
    #[arg(long, default_value_t = 160)]
    pub(super) cols: u16,
    /// ANSI stream rows.
    #[arg(long, default_value_t = 45)]
    pub(super) rows: u16,
    /// One-off command to record to completion, following `--`.
    #[arg(last = true, num_args = 1..)]
    pub(super) command: Vec<OsString>,
    /// New canonical evidence path for a one-off command.
    #[arg(long)]
    pub(super) out: Option<PathBuf>,
    /// Remove auto-allocated one-off evidence only after successful output.
    #[arg(long, conflicts_with = "out")]
    pub(super) discard_evidence: bool,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(super) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(super) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(super) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(super) at: Option<u64>,
    /// State at a unique marker label.
    #[arg(long)]
    pub(super) at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(super) include_hidden: bool,
    /// Require an unchanged live presented state for this duration.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pub(super) stable_for: Duration,
    /// Server-side deadline for a live stability request.
    #[arg(long, default_value_t = 30_000)]
    pub(super) timeout_ms: u64,
    /// Write a concise provenance header to standard error.
    #[arg(long)]
    pub(super) metadata: bool,
    /// Renderer used by image formats; native never falls back to semantic.
    #[arg(long, value_enum, default_value_t = ShowRenderer::Semantic)]
    pub(super) renderer: ShowRenderer,
    /// Optional no-replace sidecar manifest for binary image output.
    #[arg(long)]
    pub(super) manifest: Option<PathBuf>,
    /// Explicitly observe an integrity-checked incomplete event prefix offline.
    #[arg(long)]
    pub(super) recover_prefix: bool,
    /// For one-off commands, capture when exact presented text appears, then stop.
    #[arg(long, conflicts_with = "wait_regex")]
    pub(super) wait: Option<String>,
    /// For one-off commands, capture when a bounded presented regex matches, then stop.
    #[arg(long, conflicts_with = "wait")]
    pub(super) wait_regex: Option<String>,
    /// Explicitly leave a one-off recorder running and return its canonical path.
    #[arg(long)]
    pub(super) detach: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ShowFormat::Text)]
    pub(super) format: ShowFormat,
}

#[derive(Debug, Args)]
pub(super) struct TimelineArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Include synchronized-output states that were not presentation eligible.
    #[arg(long)]
    pub(super) all_states: bool,
    /// Include lossless framed VT controls and printable runs.
    #[arg(long)]
    pub(super) controls: bool,
    /// Emit one stable JSON object per line.
    #[arg(long)]
    pub(super) jsonl: bool,
    #[command(flatten)]
    pub(super) page: PageArgs,
    #[command(flatten)]
    pub(super) range: TimeRangeArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum DiffFormat {
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
pub(super) struct DiffArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Earlier exact semantic state identifier.
    #[arg(long, help_heading = "From selection")]
    pub(super) from_state: Option<u64>,
    /// Earlier exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "From selection")]
    pub(super) from_state_hash: Option<String>,
    /// Earlier state at or before this event sequence.
    #[arg(long, help_heading = "From selection")]
    pub(super) from_event_seq: Option<u64>,
    /// Earlier state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "From selection")]
    pub(super) from_at: Option<u64>,
    /// Earlier state at a unique marker.
    #[arg(long, help_heading = "From selection")]
    pub(super) from_marker: Option<String>,
    /// Later exact semantic state identifier.
    #[arg(long, help_heading = "To selection")]
    pub(super) to_state: Option<u64>,
    /// Later exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "To selection")]
    pub(super) to_state_hash: Option<String>,
    /// Later state at or before this event sequence.
    #[arg(long, help_heading = "To selection")]
    pub(super) to_event_seq: Option<u64>,
    /// Later state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "To selection")]
    pub(super) to_at: Option<u64>,
    /// Later state at a unique marker.
    #[arg(long, help_heading = "To selection")]
    pub(super) to_marker: Option<String>,
    /// Fail when any semantic or presentation value changed.
    #[arg(long)]
    pub(super) expect_no_change: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
    pub(super) format: DiffFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum ExplainFormat {
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
pub(super) struct ExplainArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long, help_heading = "Target selection")]
    pub(super) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash, help_heading = "Target selection")]
    pub(super) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long, help_heading = "Target selection")]
    pub(super) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns, help_heading = "Target selection")]
    pub(super) at: Option<u64>,
    /// State at a unique marker.
    #[arg(long, help_heading = "Target selection")]
    pub(super) marker: Option<String>,
    /// Canonical event radius before and after the target.
    #[arg(long, default_value_t = 8)]
    pub(super) context: u64,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ExplainFormat::Text)]
    pub(super) format: ExplainFormat,
    #[command(flatten)]
    pub(super) page: PageArgs,
}

#[derive(Debug, Args)]
pub(super) struct DiagnoseArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Comma-separated detector groups; omit to run every detector.
    #[arg(long, value_delimiter = ',', value_parser = parse_diagnostic_check)]
    pub(super) check: Vec<DiagnosticCheck>,
    /// Minimum changed-cell percentage for a broad repaint.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub(super) broad_change_percent: u8,
    /// Emit one stable structured report.
    #[arg(long)]
    pub(super) json: bool,
    /// Exit unsuccessfully when at least one finding is emitted.
    #[arg(long)]
    pub(super) fail_on_finding: bool,
    #[command(flatten)]
    pub(super) page: PageArgs,
}

#[derive(Debug, Args)]
pub(super) struct ReviewArgs {
    /// Completed canonical session or registered name.
    pub(super) session: Option<PathBuf>,
    /// Start at the first semantic state at or after this unique marker.
    #[arg(long)]
    pub(super) around_marker: Option<String>,
    /// Exact semantic viewport substring to find and navigate with n/N.
    #[arg(long)]
    pub(super) search: Option<String>,
    /// Export one exact semantic state to a new owner-only directory.
    #[arg(long, requires = "state")]
    pub(super) export_state: Option<PathBuf>,
    /// Exact state identifier for `--export-state`.
    #[arg(long, requires = "export_state")]
    pub(super) state: Option<u64>,
    /// Print scriptable reviewer commands and safety policy.
    #[arg(long, conflicts_with = "json")]
    pub(super) commands: bool,
    /// Print a validated read-only source summary without opening a TUI.
    #[arg(long, conflicts_with = "commands")]
    pub(super) json: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("output_representation")
        .required(true)
        .args(["raw", "strip_controls"])
))]
pub(super) struct OutputArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Write exact concatenated child PTY bytes without decoding.
    #[arg(long)]
    pub(super) raw: bool,
    /// Emit a named lossy projection retaining printable bytes and line controls.
    #[arg(long)]
    pub(super) strip_controls: bool,
    /// New no-replace JSON report path; unavailable while following.
    #[arg(long)]
    pub(super) manifest: Option<PathBuf>,
    /// Continue emitting newly durable output until the session completes.
    #[arg(long)]
    pub(super) follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    pub(super) from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    pub(super) to_event_seq: Option<u64>,
    #[command(flatten)]
    pub(super) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub(super) struct EventsArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// Comma-separated canonical event variant names.
    #[arg(long = "type", value_delimiter = ',', value_parser = parse_event_type)]
    pub(super) event_types: Vec<String>,
    /// Elide exact byte payloads while retaining their lengths and hashes.
    #[arg(long)]
    pub(super) metadata_only: bool,
    /// Continue streaming newly durable records until completion.
    #[arg(long)]
    pub(super) follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    pub(super) from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    pub(super) to_event_seq: Option<u64>,
    #[command(flatten)]
    pub(super) page: PageArgs,
    #[command(flatten)]
    pub(super) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub(super) struct PageArgs {
    /// Maximum matching records returned by default.
    #[arg(long, default_value_t = 100, value_parser = parse_page_limit)]
    pub(super) limit: usize,
    /// Zero-based matching-record offset for deterministic pagination.
    #[arg(long, default_value_t = 0)]
    pub(super) offset: usize,
    /// Return every matching record; explicit potentially-large output.
    #[arg(long)]
    pub(super) all: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(super) struct TimeRangeArgs {
    /// Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`.
    #[arg(long = "from", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "from_marker")]
    pub(super) start_monotonic_ns: Option<u64>,
    /// Inclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "start_monotonic_ns")]
    pub(super) from_marker: Option<String>,
    /// Exclusive source offset, in the same format as `--from`.
    #[arg(long = "to", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "to_marker")]
    pub(super) end_monotonic_ns: Option<u64>,
    /// Exclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "end_monotonic_ns")]
    pub(super) to_marker: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct IndexArgs {
    /// Source session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
    /// Index a hash-addressed prefix when the recording is still active.
    #[arg(long)]
    pub(super) live_snapshot: bool,
    /// Retain only states eligible for native terminal presentation.
    #[arg(long)]
    pub(super) presented_only: bool,
    #[command(flatten)]
    pub(super) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
#[command(
    override_usage = "termtrace playback [OPTIONS] --session <SESSION>\n       --timing-file <TIMING_FILE> --error-file <ERROR_FILE>\n       --geometry-file <GEOMETRY_FILE> --ready-file <READY_FILE>"
)]
pub(super) struct PlaybackArgs {
    /// Source session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
    /// Playback clock handshake path.
    #[arg(long)]
    pub(super) timing_file: PathBuf,
    /// Structured failure handshake path.
    #[arg(long)]
    pub(super) error_file: PathBuf,
    /// Playback-terminal geometry handshake path.
    #[arg(long)]
    pub(super) geometry_file: PathBuf,
    /// Final-frame readiness path.
    #[arg(long)]
    pub(super) ready_file: PathBuf,
    /// Delay before event time zero.
    #[arg(long)]
    pub(super) start_delay_ms: u64,
    /// Final-frame hold duration.
    #[arg(long)]
    pub(super) final_hold_ms: u64,
    /// Inclusive source offset used by ranged native capture.
    #[arg(long)]
    pub(super) from_ns: Option<u64>,
    /// Exclusive source offset used by ranged native capture.
    #[arg(long)]
    pub(super) to_ns: Option<u64>,
    /// Exact endpoint event used by native single-state capture.
    #[arg(long)]
    pub(super) endpoint_event_seq: Option<u64>,
    /// Inclusive byte offset within the endpoint output event.
    #[arg(long)]
    pub(super) endpoint_event_byte_offset: Option<u64>,
    /// Disable source-time sleeps while replaying an exact endpoint.
    #[arg(long)]
    pub(super) instant: bool,
}

#[derive(Debug, Args)]
pub(super) struct MarkersArgs {
    /// Completed canonical session.
    pub(super) session: PathBuf,
    /// Nominal frame rate used only for source-time mapping.
    #[arg(long, default_value_t = 24)]
    pub(super) fps: u16,
}
