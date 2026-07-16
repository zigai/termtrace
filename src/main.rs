//! Command-line entry point for Termtrace.

use std::{
    collections::VecDeque,
    env,
    ffi::OsString,
    fmt::Write as _,
    fs::{File, OpenOptions},
    io::{self, BufReader, Read as _, Write as _},
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::OpenOptionsExt as _,
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    result::Result as StdResult,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use clap::{ArgGroup, Args, CommandFactory as _, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use miette::{IntoDiagnostic as _, Result};
use termtrace::{
    Error,
    analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, diagnose, explain},
    artifact::write_secure,
    bundle::{
        BundleProfile, archive as archive_bundle, create as create_bundle, plan as plan_bundle,
        verify as verify_bundle, verify_archive as verify_bundle_archive,
    },
    capability::{CapabilityReport, CapabilityStatus, probe_capabilities},
    control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlRequest, PointerOperation,
        ProcessSignal, ScreenMode, ScreenPredicate, ScreenRegion, StateHashComparison, request,
        send_request,
    },
    diff::{SemanticDiff, diff_states, inspect_point},
    driver::serve as serve_driver,
    event::{ByteString, EVENT_LOG_VERSION, Event, EventProjection, InputKind, event_type},
    event_log::{DurabilityMode, DurabilityPolicy, EventLog},
    geometry::{inherited_geometry, run_probe},
    ghostty_vt::{
        CursorStyle, Format as GhosttyFormat, KeyModifiers, MouseButton, NamedKey, RowState,
        SemanticState,
    },
    inspection::{
        inspect_session, state_text, wait_for_completion, wait_for_output, wait_for_screen,
    },
    interop::export_asciicast,
    observation::{
        CaptureQuality, Observation, ObservationSourceKind, StatePoint, TruthLayer,
        format_ansi_stream, observe_ansi_stream, observe_one_off_session,
        observe_session_with_options, observe_validated_prefix, semantic_state_for_session,
        semantic_state_from_ansi,
    },
    output_projection::{ControlStripper, OutputProjectionQuality, OutputProjectionReport},
    playback::{PlaybackOptions, playback},
    presentation::{
        EditPlan, PresentationSource, PresentationSourceRequest, PreviewOptions,
        export as export_presentation, markers as list_markers,
        plan_with_source as plan_presentation, preview as preview_presentation,
    },
    profile::{
        ResolvedGhosttyProfile, resolve_ghostty_profile, resolved_terminal_colors,
        terminal_colors_for_profile,
    },
    provenance::{ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind},
    receipt::create_ssh_receipt,
    record::{RecordOptions, RecordOutcome, record},
    replay::{
        GHOSTTY_REVISION, IndexOptions, STATE_FORMAT_VERSION, StateSelection, StateSelector,
        format_state, index_session_with_options,
    },
    reviewer::{
        ReviewerCapabilities, export_state as export_review_state, run as run_reviewer,
        search as search_review, summarize as summarize_review,
    },
    save::{SaveFormat, save, save_screenshot_batch},
    scenario::{
        Scenario, ScenarioAttemptReport, ScenarioCaseReport, ScenarioStatus, ScenarioSuiteReport,
        ScenarioSuiteStatus, run as run_scenario,
    },
    schema::write_all as write_schemas,
    semantic_image::{
        SEMANTIC_PNG_RENDERER, SEMANTIC_SVG_RENDERER, render_diff_png, render_diff_svg, render_png,
        render_svg,
    },
    session::{
        CaptureTarget, GeometryProvenance, GeometrySource, GhosttyVisualOverrides,
        ImportProvenance, SessionGeometry, SessionLineage, SessionManifest, VerificationLevel,
        manifest_hash, read_manifest, validate_completed_log, verify_session_with_trust,
    },
    session_registry::{DetachedSessionPaths, SessionRegistry},
    timeline::{
        TimelineEntry, TimelineItem, build_timeline, parse_time_offset_ns, resolve_source_selection,
    },
    visual::{
        CaptureKind, CaptureOptions, GeometrySolveReport, capture, probe_ghostty_geometry,
        solve_ghostty_geometry,
    },
};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, registry, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[command(
    name = "termtrace",
    version,
    about = "Record native terminal pixels and every semantic terminal state"
)]
struct Cli {
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    log: tracing::Level,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report build and runtime component versions.
    Version(VersionArgs),
    /// Probe built-in and optional host capabilities.
    Doctor(DoctorArgs),
    /// Report the current bounded capability inventory.
    Capabilities(CapabilitiesArgs),
    /// Run a command in a PTY and preserve its byte-exact event stream.
    Record(RecordArgs),
    /// Execute one strict evidence-native terminal scenario.
    Test(TestArgs),
    /// Start a detached recording and publish a name after readiness.
    Start(StartArgs),
    /// Stop a named session and atomically publish an immutable successor.
    Restart(RestartArgs),
    /// Verify a completed session's structure and evidence bindings offline.
    Verify(VerifyArgs),
    /// Create a detached OpenSSH receipt for verified evidence roots.
    Sign(SignArgs),
    /// Register a friendly name for an existing canonical session.
    Register(RegisterArgs),
    /// Resolve a friendly name to its canonical session directory.
    Resolve(ResolveArgs),
    /// List registered sessions and their derived lifecycle.
    List(ListArgs),
    /// Remove only a friendly registry binding, preserving evidence.
    Remove(RemoveArgs),
    /// Verify that a live session control socket is responsive.
    Ping(SessionArgs),
    /// Print machine-readable live session status.
    Status(StatusArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
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
    /// List canonical marker positions and nominal source frames.
    Markers(MarkersArgs),
    /// Plan or export presentation edits; native Ghostty pixels are the default source.
    Video(VideoArgs),
    /// Request graceful termination of a live session child.
    Stop(StopArgs),
    /// Replay canonical events and build semantic state artifacts.
    Index(IndexArgs),
    /// Capture actual Ghostty pixels on a private headless display.
    Capture(CaptureArgs),
    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true)]
    Playback(PlaybackArgs),
    /// Internal live terminal geometry probe.
    #[command(hide = true)]
    GeometryProbe(GeometryProbeArgs),
    /// Reconstruct one semantic state as JSON or text.
    Inspect(InspectArgs),
    /// Show a truth-labeled live or completed terminal viewport.
    Show(ShowArgs),
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
    /// Generate a shell completion script from the current command graph.
    Completions(CompletionsArgs),
    /// Serve the versioned language-neutral JSONL process protocol.
    Driver,
    /// Atomically publish representations of one semantic state.
    Save(SaveArgs),
    /// Render one deterministic semantic screenshot artifact directory.
    Screenshot(ScreenshotArgs),
    /// Publish a deterministic sensitivity-labeled handoff directory.
    Bundle(BundleArgs),
    /// Verify every manifested bundle member and included evidence.
    BundleVerify(BundleVerifyArgs),
    /// Wait for exact UTF-8 text in canonical PTY output.
    Wait(WaitArgs),
    /// Copy the longest integrity-checked event prefix without modifying source.
    Recover(RecoverArgs),
    /// Generate JSON Schemas for public events, controls, and artifacts.
    Schemas(SchemaArgs),
    /// Export a lossy UTF-8 asciinema v2 interoperability cast.
    ExportAsciicast(ExportArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VerifyLevelArg {
    Structure,
    Content,
    DurabilityRecord,
    Signature,
}

#[derive(Debug, Args)]
struct VersionArgs {
    /// Emit stable structured component/build metadata.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TestArgs {
    /// One or more strict versioned scenario JSON documents.
    #[arg(num_args = 1..)]
    scenario: Vec<PathBuf>,
    /// Owner-only result, failure, and canonical evidence root.
    #[arg(long, default_value = "termtrace-results")]
    artifacts: PathBuf,
    /// Explicitly replace normalized semantic frame baselines.
    #[arg(long)]
    update_snapshots: bool,
    /// New JUnit XML result path.
    #[arg(long)]
    junit: Option<PathBuf>,
    /// Maximum concurrent fresh-session workers.
    #[arg(long, default_value_t = 1, value_parser = parse_scenario_jobs)]
    jobs: usize,
    /// Zero-based deterministic shard index.
    #[arg(long, requires = "shard_count")]
    shard_index: Option<usize>,
    /// Total deterministic shard count.
    #[arg(long, requires = "shard_index", value_parser = parse_positive_usize)]
    shard_count: Option<usize>,
    /// Fresh-session reruns after a failed attempt.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=10))]
    reruns: u8,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Emit the stable structured capability report.
    #[arg(long)]
    json: bool,
    /// Comma-separated capabilities that must be ready.
    #[arg(long, value_delimiter = ',', value_parser = parse_capability_id)]
    require: Vec<String>,
    /// Include focused remediation for blocked capabilities in text output.
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Args)]
struct CapabilitiesArgs {
    /// Emit the stable structured capability report.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BundleProfileArg {
    Screen,
    Debug,
    Forensic,
}

impl From<BundleProfileArg> for BundleProfile {
    fn from(profile: BundleProfileArg) -> Self {
        return match profile {
            BundleProfileArg::Screen => Self::Screen,
            BundleProfileArg::Debug => Self::Debug,
            BundleProfileArg::Forensic => Self::Forensic,
        };
    }
}

#[derive(Debug, Args)]
struct BundleArgs {
    /// Completed canonical session.
    session: PathBuf,
    /// Fixed content and sensitivity policy.
    #[arg(long, value_enum, default_value_t = BundleProfileArg::Screen)]
    profile: BundleProfileArg,
    /// New owner-only destination directory.
    #[arg(long)]
    out: PathBuf,
    /// Print the selected paths and sensitivities without writing.
    #[arg(long)]
    dry_run: bool,
    /// Publish a deterministic compressed archive instead of a directory.
    #[arg(long)]
    archive: bool,
}

#[derive(Debug, Args)]
struct BundleVerifyArgs {
    /// Published bundle directory.
    bundle: PathBuf,
}

impl From<VerifyLevelArg> for VerificationLevel {
    fn from(value: VerifyLevelArg) -> Self {
        return match value {
            VerifyLevelArg::Structure => Self::Structure,
            VerifyLevelArg::Content => Self::Content,
            VerifyLevelArg::DurabilityRecord => Self::DurabilityRecord,
            VerifyLevelArg::Signature => Self::Signature,
        };
    }
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Completed canonical session directory.
    session: PathBuf,
    /// Verification depth.
    #[arg(long, value_enum, default_value_t = VerifyLevelArg::Content)]
    level: VerifyLevelArg,
    /// Emit the stable machine-readable report.
    #[arg(long)]
    json: bool,
    /// OpenSSH allowed-signers file used to establish signer trust.
    #[arg(long)]
    allowed_signers: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct SignArgs {
    /// Completed canonical session directory.
    session: PathBuf,
    /// OpenSSH private key, or public key backed by an available signing agent.
    #[arg(long)]
    key: PathBuf,
    /// Identity that an allowed-signers policy will bind to this key.
    #[arg(long)]
    signer: String,
}

#[derive(Debug, Args)]
struct RegisterArgs {
    /// Friendly session name.
    name: String,
    /// Existing owner-only canonical session directory.
    #[arg(long)]
    session: PathBuf,
}

#[derive(Debug, Args)]
struct ResolveArgs {
    /// Friendly session name.
    name: String,
    /// Emit the complete registry entry as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Include completed, incomplete, stale, and unhealthy entries.
    #[arg(long)]
    all: bool,
    /// Emit the complete listing as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    /// Friendly session name to unbind.
    name: String,
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// Source session directory.
    #[arg(long)]
    session: PathBuf,
    /// New asciicast v2 output path.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct SchemaArgs {
    /// Destination directory.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct RecoverArgs {
    /// Damaged or truncated source event log.
    #[arg(long)]
    event_log: PathBuf,
    /// New recovered event-log path in an owner-only directory.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum InspectFormat {
    Json,
    JsonCompact,
    Text,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("selector")
        .multiple(false)
        .args(["state", "event_seq", "monotonic_ns"])
))]
struct InspectArgs {
    /// Source session directory.
    #[arg(long)]
    session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    state: Option<u64>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    event_seq: Option<u64>,
    /// Latest state at or before this monotonic nanosecond timestamp.
    #[arg(long)]
    monotonic_ns: Option<u64>,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = InspectFormat::Json)]
    format: InspectFormat,
    /// Restrict output to one zero-based row; JSON includes complete cell data.
    #[arg(long)]
    row: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ShowFormat {
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
enum ShowRenderer {
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
        .args(["state", "event_seq", "at", "at_marker"])
))]
struct ShowArgs {
    /// Live session name or canonical completed session path.
    session: Option<PathBuf>,
    /// ANSI/VT byte stream path, or `-` for standard input.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Promote an ANSI input stream into a new canonical import session.
    #[arg(long)]
    record_out: Option<PathBuf>,
    /// ANSI stream columns.
    #[arg(long, default_value_t = 80)]
    cols: u16,
    /// ANSI stream rows.
    #[arg(long, default_value_t = 24)]
    rows: u16,
    /// One-off command to record to completion, following `--`.
    #[arg(last = true, num_args = 1..)]
    command: Vec<OsString>,
    /// New canonical evidence path for a one-off command.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Remove auto-allocated one-off evidence only after successful output.
    #[arg(long, conflicts_with = "out")]
    discard_evidence: bool,
    /// Exact semantic state identifier.
    #[arg(long)]
    state: Option<u64>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    at: Option<u64>,
    /// State at a unique marker label.
    #[arg(long)]
    at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    include_hidden: bool,
    /// Require an unchanged live presented state for this duration.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    stable_for: Duration,
    /// Server-side deadline for a live stability request.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
    /// Write a concise provenance header to standard error.
    #[arg(long)]
    metadata: bool,
    /// Renderer used by image formats; native never falls back to semantic.
    #[arg(long, value_enum, default_value_t = ShowRenderer::Semantic)]
    renderer: ShowRenderer,
    /// Optional no-replace sidecar manifest for binary image output.
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// Explicitly observe an integrity-checked incomplete event prefix offline.
    #[arg(long)]
    recover_prefix: bool,
    /// For one-off commands, capture when exact presented text appears, then stop.
    #[arg(long, conflicts_with = "wait_regex")]
    wait: Option<String>,
    /// For one-off commands, capture when a bounded presented regex matches, then stop.
    #[arg(long, conflicts_with = "wait")]
    wait_regex: Option<String>,
    /// Explicitly leave a one-off recorder running and return its canonical path.
    #[arg(long)]
    detach: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ShowFormat::Text)]
    format: ShowFormat,
}

#[derive(Debug, Args)]
struct TimelineArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Include synchronized-output states that were not presentation eligible.
    #[arg(long)]
    all_states: bool,
    /// Include lossless framed VT controls and printable runs.
    #[arg(long)]
    controls: bool,
    /// Emit one stable JSON object per line.
    #[arg(long)]
    jsonl: bool,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DiffFormat {
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
        .args(["from_state", "from_event_seq", "from_at", "from_marker"])
))]
#[command(group(
    ArgGroup::new("diff_to")
        .required(true)
        .multiple(false)
        .args(["to_state", "to_event_seq", "to_at", "to_marker"])
))]
struct DiffArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Earlier exact semantic state identifier.
    #[arg(long)]
    from_state: Option<u64>,
    /// Earlier state at or before this event sequence.
    #[arg(long)]
    from_event_seq: Option<u64>,
    /// Earlier state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    from_at: Option<u64>,
    /// Earlier state at a unique marker.
    #[arg(long)]
    from_marker: Option<String>,
    /// Later exact semantic state identifier.
    #[arg(long)]
    to_state: Option<u64>,
    /// Later state at or before this event sequence.
    #[arg(long)]
    to_event_seq: Option<u64>,
    /// Later state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    to_at: Option<u64>,
    /// Later state at a unique marker.
    #[arg(long)]
    to_marker: Option<String>,
    /// Fail when any semantic or presentation value changed.
    #[arg(long)]
    expect_no_change: bool,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
    format: DiffFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ExplainFormat {
    Text,
    Json,
    JsonCompact,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("explain_target")
        .required(true)
        .multiple(false)
        .args(["state", "event_seq", "at", "marker"])
))]
struct ExplainArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    state: Option<u64>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    at: Option<u64>,
    /// State at a unique marker.
    #[arg(long)]
    marker: Option<String>,
    /// Canonical event radius before and after the target.
    #[arg(long, default_value_t = 8)]
    context: u64,
    /// Output representation.
    #[arg(long, value_enum, default_value_t = ExplainFormat::Text)]
    format: ExplainFormat,
}

#[derive(Debug, Args)]
struct DiagnoseArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Comma-separated detector groups; omit to run every detector.
    #[arg(long, value_delimiter = ',', value_parser = parse_diagnostic_check)]
    check: Vec<DiagnosticCheck>,
    /// Minimum changed-cell percentage for a broad repaint.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    broad_change_percent: u8,
    /// Emit one stable structured report.
    #[arg(long)]
    json: bool,
    /// Exit unsuccessfully when at least one finding is emitted.
    #[arg(long)]
    fail_on_finding: bool,
}

#[derive(Debug, Args)]
struct ReviewArgs {
    /// Completed canonical session or registered name.
    session: Option<PathBuf>,
    /// Start at the first semantic state at or after this unique marker.
    #[arg(long)]
    around_marker: Option<String>,
    /// Exact semantic viewport substring to find and navigate with n/N.
    #[arg(long)]
    search: Option<String>,
    /// Export one exact semantic state to a new owner-only directory.
    #[arg(long, requires = "state")]
    export_state: Option<PathBuf>,
    /// Exact state identifier for `--export-state`.
    #[arg(long, requires = "export_state")]
    state: Option<u64>,
    /// Print scriptable reviewer commands and safety policy.
    #[arg(long, conflicts_with = "json")]
    commands: bool,
    /// Print a validated read-only source summary without opening a TUI.
    #[arg(long, conflicts_with = "commands")]
    json: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("output_representation")
        .required(true)
        .args(["raw", "strip_controls"])
))]
struct OutputArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Write exact concatenated child PTY bytes without decoding.
    #[arg(long)]
    raw: bool,
    /// Emit a named lossy projection retaining printable bytes and line controls.
    #[arg(long)]
    strip_controls: bool,
    /// New no-replace JSON report path; unavailable while following.
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// Continue emitting newly durable output until the session completes.
    #[arg(long)]
    follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    to_event_seq: Option<u64>,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Debug, Args)]
struct EventsArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// Comma-separated canonical event variant names.
    #[arg(long = "type", value_delimiter = ',', value_parser = parse_event_type)]
    event_types: Vec<String>,
    /// Elide exact byte payloads while retaining their lengths and hashes.
    #[arg(long)]
    metadata_only: bool,
    /// Continue streaming newly durable records until completion.
    #[arg(long)]
    follow: bool,
    /// Inclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["start_monotonic_ns", "from_marker"])]
    from_event_seq: Option<u64>,
    /// Exclusive canonical event sequence.
    #[arg(long, conflicts_with_all = ["end_monotonic_ns", "to_marker"])]
    to_event_seq: Option<u64>,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Debug, Args)]
struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    shell: Shell,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SaveFormatArg {
    Txt,
    Json,
    DeepJson,
    Vt,
    Html,
    Svg,
    Png,
}

impl From<SaveFormatArg> for SaveFormat {
    fn from(value: SaveFormatArg) -> Self {
        return match value {
            SaveFormatArg::Txt => Self::Txt,
            SaveFormatArg::Json => Self::Json,
            SaveFormatArg::DeepJson => Self::DeepJson,
            SaveFormatArg::Vt => Self::Vt,
            SaveFormatArg::Html => Self::Html,
            SaveFormatArg::Svg => Self::Svg,
            SaveFormatArg::Png => Self::Png,
        };
    }
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("save_selector")
        .multiple(false)
        .args(["state", "event_seq", "at", "at_marker"])
))]
struct SaveArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// New destination directory.
    #[arg(long)]
    out: PathBuf,
    /// One or more comma-separated representations.
    #[arg(long, value_enum, value_delimiter = ',', required = true)]
    format: Vec<SaveFormatArg>,
    /// Exact semantic state identifier.
    #[arg(long)]
    state: Option<u64>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    at: Option<u64>,
    /// State at a unique marker.
    #[arg(long)]
    at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    include_hidden: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("screenshot_selector")
        .multiple(false)
        .args(["state", "event_seq", "at", "at_marker"])
))]
struct ScreenshotArgs {
    /// Completed session name or canonical path.
    session: PathBuf,
    /// New artifact directory containing `screen.svg` and its manifest.
    #[arg(long)]
    out: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    state: Vec<u64>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    event_seq: Vec<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    at: Vec<u64>,
    /// State at a unique marker.
    #[arg(long)]
    at_marker: Vec<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    include_hidden: bool,
    /// Semantic image representation.
    #[arg(long, value_enum, default_value_t = ScreenshotFormat::Png)]
    format: ScreenshotFormat,
    /// Semantic state renderer or actual Ghostty native frame.
    #[arg(long, value_enum, default_value_t = ShowRenderer::Semantic)]
    renderer: ShowRenderer,
    /// Strict whole native viewport as `WIDTHxHEIGHT`.
    #[arg(long, value_parser = parse_capture_target, conflicts_with = "fit_viewport")]
    viewport: Option<CaptureTarget>,
    /// Explicitly request the default exact-grid viewport solver.
    #[arg(long, conflicts_with = "viewport")]
    fit_viewport: bool,
    /// Emphasize that a supplied viewport is an exact evidentiary assertion.
    #[arg(long, requires = "viewport")]
    strict: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ScreenshotFormat {
    Svg,
    Png,
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
struct WaitArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Exact UTF-8 text required in the latest presented viewport.
    screen: Option<String>,
    /// Exact UTF-8 text to find in canonical output history.
    #[arg(long)]
    output: Option<String>,
    /// Exact UTF-8 text required to be absent from the presented viewport.
    #[arg(long)]
    screen_absent: Option<String>,
    /// Bounded linear-time regular expression required in the viewport.
    #[arg(long)]
    screen_regex: Option<String>,
    /// Required row substring as `ROW:TEXT`, with a zero-based row.
    #[arg(long)]
    row_contains: Option<String>,
    /// Required region substring as `LEFT,TOP,RIGHT,BOTTOM:TEXT` with half-open bounds.
    #[arg(long)]
    region_contains: Option<String>,
    /// Require the cursor at zero-based `COLUMN,ROW`.
    #[arg(long)]
    cursor_at: Option<String>,
    /// Require visible (`true`) or hidden (`false`) cursor state.
    #[arg(long)]
    cursor_visible: Option<bool>,
    /// Require one cursor shape.
    #[arg(long, value_enum)]
    cursor_shape: Option<CursorShapeArg>,
    /// Require one stable terminal mode to be enabled.
    #[arg(long, value_enum)]
    mode: Option<ScreenModeArg>,
    /// Require one stable terminal mode to be disabled.
    #[arg(long, value_enum)]
    mode_disabled: Option<ScreenModeArg>,
    /// Require the presented state to differ from this BLAKE3 hash.
    #[arg(long)]
    state_changed: Option<String>,
    /// Require the presented state to equal this BLAKE3 hash.
    #[arg(long)]
    state_unchanged: Option<String>,
    /// Require an unchanged matching presented state for this duration.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    stable_for: Duration,
    /// Maximum wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CursorShapeArg {
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
enum ScreenModeArg {
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
struct CaptureArgs {
    /// Indexed source session directory.
    #[arg(long)]
    session: PathBuf,
    /// Whole capture viewport width; defaults to recorded viewport or 1280.
    #[arg(long, requires = "height")]
    width: Option<u16>,
    /// Whole capture viewport height; defaults to recorded viewport or 720.
    #[arg(long, requires = "width")]
    height: Option<u16>,
    /// Video frame rate; native capture currently requires 24.
    #[arg(long, default_value_t = 24)]
    fps: u16,
    /// Use a matching index containing only terminal-presentable states.
    #[arg(long)]
    presented_only: bool,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Clone, Debug, Default, Args)]
struct TimeRangeArgs {
    /// Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`.
    #[arg(long = "from", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "from_marker")]
    start_monotonic_ns: Option<u64>,
    /// Inclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "start_monotonic_ns")]
    from_marker: Option<String>,
    /// Exclusive source offset, in the same format as `--from`.
    #[arg(long = "to", value_name = "OFFSET", value_parser = parse_time_offset_ns, conflicts_with = "to_marker")]
    end_monotonic_ns: Option<u64>,
    /// Exclusive source marker; the label must occur exactly once.
    #[arg(long, value_name = "LABEL", conflicts_with = "end_monotonic_ns")]
    to_marker: Option<String>,
}

#[derive(Debug, Args)]
struct IndexArgs {
    /// Source session directory.
    #[arg(long)]
    session: PathBuf,
    /// Index a hash-addressed prefix when the recording is still active.
    #[arg(long)]
    live_snapshot: bool,
    /// Retain only states eligible for native terminal presentation.
    #[arg(long)]
    presented_only: bool,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Debug, Args)]
struct GeometryProbeArgs {
    /// New file receiving the measured terminal geometry.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct PlaybackArgs {
    /// Source session directory.
    #[arg(long)]
    session: PathBuf,
    /// Playback clock handshake path.
    #[arg(long)]
    timing_file: PathBuf,
    /// Structured failure handshake path.
    #[arg(long)]
    error_file: PathBuf,
    /// Final-frame readiness path.
    #[arg(long)]
    ready_file: PathBuf,
    /// Delay before event time zero.
    #[arg(long)]
    start_delay_ms: u64,
    /// Final-frame hold duration.
    #[arg(long)]
    final_hold_ms: u64,
    /// Inclusive source offset used by ranged native capture.
    #[arg(long)]
    from_ns: Option<u64>,
    /// Exclusive source offset used by ranged native capture.
    #[arg(long)]
    to_ns: Option<u64>,
    /// Exact endpoint event used by native single-state capture.
    #[arg(long)]
    endpoint_event_seq: Option<u64>,
    /// Inclusive byte offset within the endpoint output event.
    #[arg(long)]
    endpoint_event_byte_offset: Option<u64>,
    /// Disable source-time sleeps while replaying an exact endpoint.
    #[arg(long)]
    instant: bool,
}

#[derive(Debug, Args)]
struct SessionArgs {
    /// Live session name or canonical path.
    session: PathBuf,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Session name or canonical path.
    session: PathBuf,
    /// Wait for durable completion instead of returning live counters.
    #[arg(long)]
    wait_complete: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct StopArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Wait for and validate durable completion after stop is acknowledged.
    #[arg(long)]
    wait: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("input")
        .required(true)
        .multiple(false)
        .args(["atoms", "stdin"])
))]
struct SendArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Named keys, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH atoms.
    #[arg(value_name = "ATOM", num_args = 1..)]
    atoms: Vec<String>,
    /// Read one exact raw burst from standard input.
    #[arg(long)]
    stdin: bool,
    /// Delay between atoms; paced text is split by Unicode scalar.
    #[arg(long, value_parser = parse_pace_duration, default_value = "0ms")]
    pace: Duration,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    intent: Option<String>,
    /// Apply every input atom only if the latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Debug, Args)]
struct ResizeArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Terminal columns.
    #[arg(long)]
    cols: u16,
    /// Terminal rows.
    #[arg(long)]
    rows: u16,
    /// Total pixel width; defaults to columns multiplied by 9.
    #[arg(long)]
    pixel_width: Option<u16>,
    /// Total pixel height; defaults to rows multiplied by 18.
    #[arg(long)]
    pixel_height: Option<u16>,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FocusDirection {
    In,
    Out,
}

#[derive(Debug, Args)]
struct FocusArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Focus transition to report.
    #[arg(value_enum)]
    direction: FocusDirection,
    /// Reject when reporting is disabled instead of returning a semantic no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliPointerButton {
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
struct PointerArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    #[command(subcommand)]
    operation: PointerCommand,
}

#[derive(Debug, Subcommand)]
enum PointerCommand {
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
struct PointerClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    button: CliPointerButton,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
struct PointerMultiClickArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    cell: (u16, u16),
    /// Button to click.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    button: CliPointerButton,
    /// Number of complete clicks.
    #[arg(long, value_parser = clap::value_parser!(u8).range(2..=4))]
    count: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
struct PointerDragArgs {
    /// Zero-based COLUMN,ROW press cell.
    #[arg(long, value_parser = parse_cell)]
    from: (u16, u16),
    /// Zero-based COLUMN,ROW release cell.
    #[arg(long, value_parser = parse_cell)]
    to: (u16, u16),
    /// Button held throughout the drag.
    #[arg(long, value_enum, default_value_t = CliPointerButton::Left)]
    button: CliPointerButton,
    /// Number of motion samples, including the destination.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=100))]
    steps: u8,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
struct PointerScrollArgs {
    /// Zero-based COLUMN,ROW cell.
    #[arg(long, value_parser = parse_cell)]
    cell: (u16, u16),
    /// Signed lines: negative is up and positive is down.
    #[arg(long)]
    lines: i16,
    /// Reject when tracking suppresses output instead of returning a no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
    #[command(flatten)]
    postcondition: PointerPostconditionArgs,
}

#[derive(Debug, Args)]
struct PointerPostconditionArgs {
    /// After applying input, wait until the presented screen contains this text.
    #[arg(long, conflicts_with = "wait_screen_absent")]
    wait_screen: Option<String>,
    /// After applying input, wait until the presented screen excludes this text.
    #[arg(long, conflicts_with = "wait_screen")]
    wait_screen_absent: Option<String>,
    /// Require one unchanged matching presented state for this duration.
    #[arg(long, default_value = "0ms", value_parser = parse_pace_duration)]
    stable_for: Duration,
    /// Bounded postcondition deadline.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct ActivateArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Exact visible row-contained text to resolve.
    #[arg(long)]
    text: String,
    /// Require exactly one visible match.
    #[arg(long)]
    unique: bool,
    /// Reject when mouse reporting suppresses output instead of returning a no-op.
    #[arg(long)]
    strict: bool,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliSignal {
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
struct SignalArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Allow-listed signal.
    #[arg(long, value_enum)]
    signal: CliSignal,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Debug, Args)]
struct MarkArgs {
    /// Live session name or canonical path.
    session: PathBuf,
    /// Marker label.
    #[arg(long)]
    label: String,
    /// Apply only if the actor's latest presented state has this BLAKE3 hash.
    #[arg(long = "if-state")]
    expected_presented_state_hash: Option<String>,
}

#[derive(Debug, Args)]
struct MarkersArgs {
    /// Completed canonical session.
    session: PathBuf,
    /// Nominal frame rate used only for source-time mapping.
    #[arg(long, default_value_t = 24)]
    fps: u16,
}

#[derive(Debug, Args)]
struct VideoArgs {
    #[command(subcommand)]
    command: VideoCommand,
}

#[derive(Debug, Subcommand)]
enum VideoCommand {
    /// Resolve clips, speeds, and holds without rendering.
    Plan(VideoPlanArgs),
    /// Render an atomic contact sheet and low-resolution review video.
    Preview(VideoPreviewArgs),
    /// Export an H.264 MP4 and frame provenance into a new directory.
    Export(VideoExportArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum VideoSourceArg {
    /// Verified pixels captured from the real Ghostty renderer.
    Native,
    /// Deterministic pixels generated from canonical semantic replay.
    Semantic,
}

#[derive(Debug, Args)]
struct VideoSourceArgs {
    /// Presentation source; native Ghostty pixels remain the default.
    #[arg(long, value_enum, default_value_t = VideoSourceArg::Native)]
    source: VideoSourceArg,
    /// Semantic source frame rate; invalid for native capture.
    #[arg(long)]
    fps: Option<u16>,
}

#[derive(Debug, Args)]
struct VideoPreviewArgs {
    /// Completed session; native source additionally requires a verified capture.
    session: PathBuf,
    #[command(flatten)]
    source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    edit: PathBuf,
    /// New no-replace preview artifact directory.
    #[arg(long)]
    out: PathBuf,
    /// Storyboard tile columns.
    #[arg(long, default_value_t = 4)]
    columns: u8,
    /// Width of each storyboard tile.
    #[arg(long, default_value_t = 320)]
    tile_width: u16,
    /// Maximum storyboard frames.
    #[arg(long, default_value_t = 12)]
    max_frames: u16,
    /// Retain only frames carrying configured change visualization.
    #[arg(long)]
    change_only: bool,
    /// Low-resolution preview width.
    #[arg(long, default_value_t = 640)]
    preview_width: u16,
}

#[derive(Debug, Args)]
struct VideoPlanArgs {
    /// Completed session; native source additionally requires a verified capture.
    session: PathBuf,
    #[command(flatten)]
    source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    edit: PathBuf,
}

#[derive(Debug, Args)]
struct VideoExportArgs {
    /// Completed session; native source additionally requires a verified capture.
    session: PathBuf,
    #[command(flatten)]
    source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    edit: PathBuf,
    /// New no-replace presentation artifact directory.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct RecordArgs {
    /// New owner-only session directory.
    #[arg(long)]
    out: PathBuf,

    /// Do not forward standard input to the child PTY.
    #[arg(long)]
    no_input: bool,

    /// Friendly invocation context supplied by detached start.
    #[arg(long, hide = true)]
    session_alias: Option<String>,

    /// Completed predecessor used to construct an exact restart successor.
    #[arg(long, hide = true)]
    restart_source: Option<PathBuf>,

    /// Base64-safe JSON restart override descriptor supplied by the supervisor.
    #[arg(long, hide = true)]
    restart_overrides: Option<String>,

    /// Internal source kind for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    import_source_kind: Option<String>,
    /// Internal exact byte count for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    import_source_byte_count: Option<u64>,
    /// Internal exact BLAKE3 for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    import_source_blake3: Option<String>,
    /// Internal original source path bytes for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    import_source: Option<OsString>,

    #[command(flatten)]
    recording: RecordingArgs,
}

#[derive(Debug, Args)]
struct StartArgs {
    /// New friendly session name.
    name: String,

    /// Maximum time to wait for manifest and control readiness.
    #[arg(long, default_value_t = 10_000)]
    startup_timeout_ms: u64,

    #[command(flatten)]
    recording: RecordingArgs,
}

#[derive(Debug, Args)]
struct RestartArgs {
    /// Existing friendly session name.
    name: String,

    /// Maximum time for predecessor completion and successor readiness.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,

    /// Override the successor working directory.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Override terminal columns while retaining the predecessor when omitted.
    #[arg(long)]
    cols: Option<u16>,
    /// Override terminal rows while retaining the predecessor when omitted.
    #[arg(long)]
    rows: Option<u16>,
    /// Override cell width in pixels.
    #[arg(long)]
    cell_width: Option<u16>,
    /// Override cell height in pixels.
    #[arg(long)]
    cell_height: Option<u16>,
    /// Override the terminal identity exposed to the child.
    #[arg(long)]
    term: Option<String>,
    /// Override the canonical output persistence policy.
    #[arg(long, value_enum)]
    durability: Option<DurabilityModeArg>,
    /// Override the bounded maximum unsynchronized interval.
    #[arg(long)]
    durability_max_interval_ms: Option<u64>,
    /// Override the bounded maximum unsynchronized byte count.
    #[arg(long)]
    durability_max_uncommitted_bytes: Option<u64>,
    /// Override the execution provenance policy.
    #[arg(long, value_enum)]
    provenance: Option<ProvenancePolicyArg>,
    /// Override custom environment rules.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    provenance_environment: Vec<(String, ProvenanceCapture)>,
    /// Override the executable and arguments; options must precede the executable.
    #[arg(num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<OsString>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartOverrides {
    cwd: Option<ByteString>,
    cols: Option<u16>,
    rows: Option<u16>,
    cell_width: Option<u16>,
    cell_height: Option<u16>,
    term: Option<String>,
    durability: Option<DurabilityModeArg>,
    durability_max_interval_ms: Option<u64>,
    durability_max_uncommitted_bytes: Option<u64>,
    provenance: Option<ProvenancePolicyArg>,
    provenance_environment: Vec<(String, ProvenanceCapture)>,
    command: Option<Vec<ByteString>>,
}

#[derive(Debug, Args)]
struct RecordingArgs {
    /// Child working directory; defaults to the caller's current directory.
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Geometry selection policy.
    #[arg(long, value_enum, default_value_t = GeometryMode::Manual)]
    geometry: GeometryMode,

    /// Intended capture viewport as WIDTHxHEIGHT; required for fitted geometry.
    #[arg(long, value_parser = parse_capture_target)]
    viewport: Option<CaptureTarget>,

    /// Ghostty config file used instead of the default user config.
    #[arg(long)]
    ghostty_config: Option<PathBuf>,

    /// Override the regular Ghostty font family.
    #[arg(long)]
    font_family: Option<String>,

    /// Override the Ghostty font size in points.
    #[arg(long, value_parser = parse_font_size)]
    font_size: Option<String>,

    /// Override symmetric Ghostty horizontal padding in pixels.
    #[arg(long)]
    padding_x: Option<u16>,

    /// Override symmetric Ghostty vertical padding in pixels.
    #[arg(long)]
    padding_y: Option<u16>,

    /// Override the Ghostty foreground color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    foreground: Option<String>,

    /// Override the Ghostty background color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    background: Option<String>,

    /// Terminal columns used by manual geometry.
    #[arg(long, default_value_t = 160)]
    cols: u16,

    /// Terminal rows used by manual geometry.
    #[arg(long, default_value_t = 40)]
    rows: u16,

    /// Manual cell width, or inherited-geometry fallback, in pixels.
    #[arg(long, default_value_t = 9)]
    cell_width: u16,

    /// Manual cell height, or inherited-geometry fallback, in pixels.
    #[arg(long, default_value_t = 18)]
    cell_height: u16,

    /// Terminal identity exposed to the child.
    #[arg(long, default_value = "xterm-ghostty")]
    term: String,

    /// Canonical output persistence policy.
    #[arg(long, value_enum, default_value_t = DurabilityModeArg::Bounded)]
    durability: DurabilityModeArg,

    /// Maximum unsynchronized interval in bounded mode.
    #[arg(long, default_value_t = 50)]
    durability_max_interval_ms: u64,

    /// Maximum unsynchronized PTY bytes in bounded mode.
    #[arg(long, default_value_t = 65_536)]
    durability_max_uncommitted_bytes: u64,

    /// Execution provenance collection policy.
    #[arg(long, value_enum, default_value_t = ProvenancePolicyArg::Minimal)]
    provenance: ProvenancePolicyArg,

    /// Custom environment rule as NAME=value|hash|name|not-captured.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    provenance_environment: Vec<(String, ProvenanceCapture)>,

    /// Executable and arguments; options must precede the executable.
    #[arg(
        required = true,
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    command: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GeometryMode {
    Manual,
    Inherit,
    Fit,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
enum DurabilityModeArg {
    Strict,
    Bounded,
    Throughput,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
enum ProvenancePolicyArg {
    Minimal,
    Reproducible,
    Custom,
}

impl From<ProvenancePolicyArg> for ProvenancePolicyKind {
    fn from(value: ProvenancePolicyArg) -> Self {
        return match value {
            ProvenancePolicyArg::Minimal => Self::Minimal,
            ProvenancePolicyArg::Reproducible => Self::Reproducible,
            ProvenancePolicyArg::Custom => Self::Custom,
        };
    }
}

impl DurabilityModeArg {
    fn policy(self, maximum_interval_ms: u64, maximum_uncommitted_bytes: u64) -> DurabilityPolicy {
        return match self {
            Self::Strict => DurabilityPolicy {
                mode: DurabilityMode::Strict,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
            Self::Bounded => DurabilityPolicy {
                mode: DurabilityMode::Bounded,
                maximum_interval_ms: Some(maximum_interval_ms),
                maximum_uncommitted_bytes: Some(maximum_uncommitted_bytes),
            },
            Self::Throughput => DurabilityPolicy {
                mode: DurabilityMode::Throughput,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
        };
    }
}

fn init_diagnostics(level: tracing::Level) {
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    registry()
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .with(fmt::layer().with_target(false).with_writer(io::stderr))
        .init();
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    init_diagnostics(cli.log);
    return dispatch(cli.command);
}

fn dispatch(command: Command) -> Result<ExitCode> {
    match command {
        Command::Version(arguments) => return run_version(arguments),
        Command::Doctor(arguments) => return run_doctor(arguments),
        Command::Capabilities(arguments) => return run_capabilities(arguments),
        Command::Record(arguments) => return run_record(arguments),
        Command::Test(arguments) => return run_test(arguments),
        Command::Start(arguments) => return run_start(arguments),
        Command::Restart(arguments) => return run_restart(arguments),
        Command::Verify(arguments) => return run_verify(arguments),
        Command::Sign(arguments) => return run_sign(arguments),
        Command::Register(arguments) => return run_register(arguments),
        Command::Resolve(arguments) => return run_resolve(arguments),
        Command::List(arguments) => return run_list(arguments),
        Command::Remove(arguments) => return run_remove(arguments),
        Command::Ping(arguments) => return run_control(arguments.session, ControlCommand::Ping),
        Command::Status(arguments) => return run_status(arguments),
        Command::Send(arguments) => return run_send(arguments),
        Command::Focus(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Focus {
                    focused: matches!(arguments.direction, FocusDirection::In),
                    strict: arguments.strict,
                },
                arguments.expected_presented_state_hash,
            );
        }
        Command::Pointer(arguments) => return run_pointer(arguments),
        Command::Activate(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Activate {
                    text: arguments.text,
                    unique: arguments.unique,
                    strict: arguments.strict,
                },
                arguments.expected_presented_state_hash,
            );
        }
        Command::Resize(arguments) => return run_resize(arguments),
        Command::Signal(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Signal {
                    signal: arguments.signal.into(),
                },
                arguments.expected_presented_state_hash,
            );
        }
        Command::Mark(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Mark {
                    label: arguments.label,
                },
                arguments.expected_presented_state_hash,
            );
        }
        Command::Markers(arguments) => return run_markers(arguments),
        Command::Video(arguments) => return run_video(arguments),
        Command::Stop(arguments) => return run_stop(arguments),
        Command::Index(arguments) => return run_index(arguments),
        Command::Capture(arguments) => return run_capture(arguments),
        Command::Playback(arguments) => return run_playback(arguments),
        Command::GeometryProbe(arguments) => {
            run_probe(&arguments.out)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Inspect(arguments) => return run_inspect(arguments),
        Command::Show(arguments) => return run_show(arguments),
        Command::Timeline(arguments) => return run_timeline(arguments),
        Command::Diff(arguments) => return run_diff(arguments),
        Command::Explain(arguments) => return run_explain(arguments),
        Command::Diagnose(arguments) => return run_diagnose(arguments),
        Command::Review(arguments) => return run_review(arguments),
        Command::Output(arguments) => return run_output(arguments),
        Command::Events(arguments) => return run_events(arguments),
        Command::Completions(arguments) => return run_completions(arguments),
        Command::Driver => return run_driver(),
        Command::Save(arguments) => return run_save(arguments),
        Command::Screenshot(arguments) => return run_screenshot(arguments),
        Command::Bundle(arguments) => return run_bundle(arguments),
        Command::BundleVerify(arguments) => return run_bundle_verify(arguments),
        Command::Wait(arguments) => return run_wait(arguments),
        Command::Recover(arguments) => return run_recover(arguments),
        Command::Schemas(arguments) => return run_schemas(arguments),
        Command::ExportAsciicast(arguments) => {
            export_asciicast(arguments.session, arguments.out)?;
            return Ok(ExitCode::SUCCESS);
        }
    }
}

fn run_markers(arguments: MarkersArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    return write_json_line(&list_markers(session, arguments.fps)?);
}

fn run_review(arguments: ReviewArgs) -> Result<ExitCode> {
    if arguments.commands {
        return write_json_line(&ReviewerCapabilities::default());
    }
    let session = arguments.session.ok_or_else(|| {
        return miette::miette!("review requires SESSION unless --commands is used");
    })?;
    let session = resolve_session_ref(session)?;
    if let (Some(destination), Some(state_id)) = (&arguments.export_state, arguments.state) {
        return write_json_line(&export_review_state(session, state_id, destination)?);
    }
    if arguments.json {
        if let Some(query) = &arguments.search {
            return write_json_line(&search_review(session, query)?);
        }
        return write_json_line(&summarize_review(
            session,
            arguments.around_marker.as_deref(),
        )?);
    }
    run_reviewer(
        session,
        arguments.around_marker.as_deref(),
        arguments.search.as_deref(),
    )?;
    return Ok(ExitCode::SUCCESS);
}

fn run_video(arguments: VideoArgs) -> Result<ExitCode> {
    return match arguments.command {
        VideoCommand::Plan(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            write_json_line(&plan_presentation(session, &edit, source)?)
        }
        VideoCommand::Preview(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            let plan = plan_presentation(session, &edit, source)?;
            let outcome = preview_presentation(
                &plan,
                arguments.out,
                PreviewOptions {
                    columns: arguments.columns,
                    tile_width: arguments.tile_width,
                    max_frames: arguments.max_frames,
                    change_only: arguments.change_only,
                    preview_width: arguments.preview_width,
                },
            )?;
            write_json_line(&serde_json::json!({
                "kind": "presentation_preview",
                "directory": outcome.directory,
                "contact_sheet": outcome.contact_sheet,
                "video": outcome.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "source": match &plan.source { PresentationSource::Native { .. } => "native", PresentationSource::Semantic { .. } => "semantic" },
            }))
        }
        VideoCommand::Export(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            let edit = EditPlan::read(arguments.edit)?;
            let source = video_source_request(arguments.source)?;
            let plan = plan_presentation(session, &edit, source)?;
            let outcome = export_presentation(&plan, arguments.out)?;
            write_json_line(&serde_json::json!({
                "kind": "presentation",
                "directory": outcome.directory,
                "video": outcome.video,
                "edit_plan_hash": plan.edit_plan_hash,
                "output_frame_count": plan.output_frame_count,
                "source": match &plan.source { PresentationSource::Native { .. } => "native", PresentationSource::Semantic { .. } => "semantic" },
            }))
        }
    };
}

fn video_source_request(arguments: VideoSourceArgs) -> Result<PresentationSourceRequest> {
    return match arguments.source {
        VideoSourceArg::Native if arguments.fps.is_some() => Err(miette::miette!(
            "--fps configures semantic video only; native FPS comes from the capture manifest"
        )),
        VideoSourceArg::Native => Ok(PresentationSourceRequest::Native),
        VideoSourceArg::Semantic => {
            let fps = arguments.fps.unwrap_or(24);
            if !(1..=120).contains(&fps) {
                return Err(miette::miette!(
                    "semantic video FPS must be between 1 and 120"
                ));
            }
            Ok(PresentationSourceRequest::Semantic { fps })
        }
    };
}

fn run_test(arguments: TestArgs) -> Result<ExitCode> {
    let shard = match (arguments.shard_index, arguments.shard_count) {
        (Some(index), Some(count)) if index < count => Some((index, count)),
        (Some(index), Some(count)) => {
            return Err(miette::miette!(
                "scenario shard index {index} must be less than shard count {count}"
            ));
        }
        (None, None) => None,
        _ => {
            return Err(miette::miette!(
                "scenario sharding requires index and count"
            ));
        }
    };
    let mut paths = arguments.scenario;
    paths.sort();
    paths.dedup();
    let tasks = paths
        .into_iter()
        .map(|path| {
            let scenario = Scenario::read(&path)?;
            return scenario
                .expand_matrix()?
                .into_iter()
                .map(|(label, scenario)| {
                    let identity = if label.is_empty() {
                        path.clone()
                    } else {
                        PathBuf::from(format!("{}#{label}", path.display()))
                    };
                    return Ok((identity, scenario));
                })
                .collect::<Result<Vec<_>>>();
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(index, _task)| {
            return shard.is_none_or(|(shard_index, shard_count)| {
                return index % shard_count == shard_index;
            });
        })
        .map(|(_index, task)| return task)
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Err(miette::miette!(
            "scenario selection produced an empty shard"
        ));
    }
    let binary = env::current_exe().into_diagnostic()?;
    let task_count = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (sender, receiver) = mpsc::channel();
    let mut cases = vec![None; task_count];
    thread::scope(|scope| -> Result<()> {
        for _worker in 0..arguments.jobs.min(task_count) {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let binary = binary.clone();
            let artifacts = arguments.artifacts.clone();
            scope.spawn(move || {
                loop {
                    let task = {
                        let mut queue = match queue.lock() {
                            Ok(queue) => queue,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        queue.pop_front()
                    };
                    let Some((path, scenario)) = task else {
                        return;
                    };
                    let result = run_scenario_case(
                        path,
                        scenario,
                        &binary,
                        &artifacts,
                        arguments.update_snapshots,
                        arguments.reruns,
                    )
                    .map_err(|error| return error.to_string());
                    if sender.send(result).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        for slot in &mut cases {
            let case = receiver
                .recv()
                .into_diagnostic()?
                .map_err(|message| return miette::miette!(message))?;
            *slot = Some(case);
        }
        return Ok(());
    })?;
    let mut cases = cases.into_iter().flatten().collect::<Vec<_>>();
    cases.sort_by(|left, right| return left.scenario.cmp(&right.scenario));
    let status = if cases.iter().all(|case| {
        return case
            .attempts
            .last()
            .is_some_and(|attempt| return attempt.report.status == ScenarioStatus::Passed);
    }) {
        ScenarioSuiteStatus::Passed
    } else {
        ScenarioSuiteStatus::Failed
    };
    let report = ScenarioSuiteReport {
        version: 1,
        status,
        jobs: arguments.jobs,
        shard_index: shard.map(|value| return value.0),
        shard_count: shard.map(|value| return value.1),
        cases,
    };
    if let Some(path) = &arguments.junit {
        write_scenario_junit(path, &report)?;
    }
    write_json_line(&report)?;
    return Ok(if status == ScenarioSuiteStatus::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    });
}

fn run_scenario_case(
    path: PathBuf,
    scenario: Scenario,
    binary: &Path,
    artifacts: &Path,
    update_snapshots: bool,
    reruns: u8,
) -> Result<ScenarioCaseReport> {
    let mut attempts = Vec::new();
    for attempt in 1..=u32::from(reruns).saturating_add(1) {
        let report = run_scenario(
            &scenario,
            binary,
            artifacts.join("sessions"),
            artifacts.join("failures"),
            update_snapshots,
        )?;
        let passed = report.status == ScenarioStatus::Passed;
        attempts.push(ScenarioAttemptReport { attempt, report });
        if passed {
            break;
        }
    }
    return Ok(ScenarioCaseReport {
        scenario: path,
        attempts,
    });
}

fn write_scenario_junit(path: &Path, report: &ScenarioSuiteReport) -> Result<()> {
    let failure_count = report
        .cases
        .iter()
        .filter(|case| {
            return case
                .attempts
                .last()
                .is_none_or(|attempt| return attempt.report.status == ScenarioStatus::Failed);
        })
        .count();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"termtrace\" tests=\"{}\" failures=\"{failure_count}\">",
        report.cases.len()
    );
    for case in &report.cases {
        let final_attempt = case.attempts.last();
        let name = final_attempt.map_or_else(
            || return case.scenario.display().to_string(),
            |attempt| return attempt.report.name.clone(),
        );
        write!(xml, "\n  <testcase name=\"{}\">", xml_escape(&name)).into_diagnostic()?;
        if let Some(failure) =
            final_attempt.and_then(|attempt| return attempt.report.failure.as_ref())
        {
            write!(
                xml,
                "\n    <failure message=\"{}\">step {} ({})</failure>\n  ",
                xml_escape(&failure.message),
                failure.step_index,
                xml_escape(&failure.step_kind)
            )
            .into_diagnostic()?;
        }
        write!(xml, "</testcase>").into_diagnostic()?;
    }
    writeln!(xml, "\n</testsuite>").into_diagnostic()?;
    write_secure(path, xml.as_bytes())?;
    return Ok(());
}

fn xml_escape(value: &str) -> String {
    return value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
}

fn run_version(arguments: VersionArgs) -> Result<ExitCode> {
    if arguments.json {
        return write_json_line(&serde_json::json!({
            "termtrace_version": env!("CARGO_PKG_VERSION"),
            "build_revision": env!("TERMTRACE_BUILD_REVISION"),
            "ghostty_vt_revision": GHOSTTY_REVISION,
            "event_log_version": EVENT_LOG_VERSION,
            "state_format_version": STATE_FORMAT_VERSION,
            "control_protocol_version": CONTROL_PROTOCOL_VERSION,
            "target_os": env::consts::OS,
            "target_arch": env::consts::ARCH,
        }));
    }
    return write_stdout(format!(
        "termtrace {}\nbuild {}\nlibghostty-vt {GHOSTTY_REVISION}\nevent-log v{EVENT_LOG_VERSION}\nstate-format v{STATE_FORMAT_VERSION}\ncontrol-protocol v{CONTROL_PROTOCOL_VERSION}\n",
        env!("CARGO_PKG_VERSION"),
        env!("TERMTRACE_BUILD_REVISION")
    ));
}

fn run_doctor(arguments: DoctorArgs) -> Result<ExitCode> {
    let report = probe_capabilities();
    if arguments.json {
        write_json_line(&report)?;
    } else {
        write_stdout(capability_text(&report, arguments.explain)?)?;
    }
    if arguments
        .require
        .iter()
        .any(|required| return !report.is_ready(required))
    {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_capabilities(arguments: CapabilitiesArgs) -> Result<ExitCode> {
    let report = probe_capabilities();
    if arguments.json {
        return write_json_line(&report);
    }
    return write_stdout(capability_text(&report, false)?);
}

fn capability_text(report: &CapabilityReport, explain: bool) -> Result<String> {
    let mut output = String::new();
    for capability in &report.capabilities {
        writeln!(
            output,
            "{:<20} {:<7} {}",
            capability.id,
            match capability.status {
                CapabilityStatus::Ready => "ready",
                CapabilityStatus::Blocked => "blocked",
            },
            capability.detail
        )
        .into_diagnostic()?;
        if explain && let Some(remediation) = &capability.remediation {
            writeln!(output, "  fix: {remediation}").into_diagnostic()?;
        }
    }
    return Ok(output);
}

fn run_verify(arguments: VerifyArgs) -> Result<ExitCode> {
    if arguments.allowed_signers.is_some() && !matches!(arguments.level, VerifyLevelArg::Signature)
    {
        return Err(miette::miette!(
            "--allowed-signers requires --level signature"
        ));
    }
    let report = verify_session_with_trust(
        arguments.session,
        arguments.level.into(),
        arguments.allowed_signers.as_deref(),
    );
    let valid = report.valid;
    if arguments.json {
        write_json_line(&report)?;
    } else {
        let status = if valid { "valid" } else { "invalid" };
        let mut output = format!(
            "{status}: {} ({:?})\n",
            report.session.display(),
            report.level
        );
        for finding in &report.findings {
            writeln!(output, "{}: {}", finding.code, finding.message).into_diagnostic()?;
        }
        write_stdout(output)?;
    }
    if valid {
        return Ok(ExitCode::SUCCESS);
    }
    return Ok(ExitCode::FAILURE);
}

fn run_sign(arguments: SignArgs) -> Result<ExitCode> {
    let receipt = create_ssh_receipt(&arguments.session, &arguments.key, &arguments.signer)?;
    return write_json_line(&serde_json::json!({
        "receipt": receipt,
        "scheme": "openssh",
        "signer_key_id": arguments.signer,
    }));
}

fn run_register(arguments: RegisterArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.register(&arguments.name, &arguments.session)?;
    return write_json_line(&entry);
}

fn run_resolve(arguments: ResolveArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.resolve(&arguments.name)?;
    if arguments.json {
        return write_json_line(&entry);
    }
    let mut path = entry.session_path().into_os_string().into_vec();
    path.push(b'\n');
    return write_stdout(path);
}

fn run_list(arguments: ListArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entries = registry.list(arguments.all)?;
    if arguments.json {
        return write_json_line(&entries);
    }
    let mut output = String::new();
    for entry in entries {
        let path = PathBuf::from(OsString::from_vec(
            entry.canonical_session.as_bytes().to_vec(),
        ));
        writeln!(
            output,
            "{}\t{}\t{}\t{}",
            entry.alias,
            entry.lifecycle.as_str(),
            entry.session_id,
            path.display()
        )
        .into_diagnostic()?;
    }
    return write_stdout(output);
}

fn run_remove(arguments: RemoveArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.remove(&arguments.name)?;
    return write_json_line(&serde_json::json!({
        "alias": entry.alias,
        "canonical_session": entry.canonical_session,
        "session_id": entry.session_id,
        "evidence_preserved": true,
    }));
}

fn run_index(arguments: IndexArgs) -> Result<ExitCode> {
    if !arguments.live_snapshot && !arguments.session.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: arguments.session,
        }
        .into());
    }
    let (start_monotonic_ns, end_monotonic_ns) =
        resolve_cli_range(&arguments.session, &arguments.range)?;
    let state_selection = selection(arguments.presented_only);
    let outcome = index_session_with_options(
        arguments.session,
        IndexOptions {
            start_monotonic_ns,
            end_monotonic_ns,
            live_snapshot: arguments.live_snapshot,
            state_selection,
        },
    )?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "state_count": outcome.state_count,
        "semantic_state_count": outcome.semantic_state_count,
        "source_complete": outcome.source_complete,
        "state_selection": outcome.state_selection,
        "final_state_hash": outcome.final_state_hash,
    }));
}

fn run_capture(arguments: CaptureArgs) -> Result<ExitCode> {
    let (start_monotonic_ns, end_monotonic_ns) =
        resolve_cli_range(&arguments.session, &arguments.range)?;
    let (target, geometry_solver) = match (arguments.width, arguments.height) {
        (Some(width), Some(height)) => (CaptureTarget { width, height }, None),
        (None, None) => {
            let manifest = read_manifest(&arguments.session)?;
            let initial = manifest
                .geometry_provenance
                .capture_target
                .unwrap_or_else(|| {
                    return CaptureTarget {
                        width: manifest.geometry.pixel_width.saturating_add(128).max(320),
                        height: manifest.geometry.pixel_height.saturating_add(128).max(200),
                    };
                });
            let profile = capture_profile(&arguments.session, &manifest)?;
            let report = solve_ghostty_geometry(
                manifest.geometry.cols,
                manifest.geometry.rows,
                initial,
                &profile,
            )?;
            (report.selected, Some(report))
        }
        _ => {
            return Err(miette::miette!(
                "--width and --height must be supplied together"
            ));
        }
    };
    let outcome = capture(&CaptureOptions {
        session_directory: arguments.session,
        width: target.width,
        height: target.height,
        fps: arguments.fps,
        start_monotonic_ns,
        end_monotonic_ns,
        state_selection: selection(arguments.presented_only),
        geometry_solver,
        kind: CaptureKind::Video,
        destination: None,
    })?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "video": outcome.video,
        "screenshot": outcome.screenshot,
    }));
}

fn capture_profile(session: &Path, manifest: &SessionManifest) -> Result<ResolvedGhosttyProfile> {
    let Some(provenance) = &manifest.ghostty_visual else {
        return Ok(resolve_ghostty_profile(
            None,
            GhosttyVisualOverrides::default(),
        )?);
    };
    let path = session.join(&provenance.effective_config);
    let effective_config = fs_err::read(&path).into_diagnostic()?;
    let actual_hash = blake3::hash(&effective_config).to_hex().to_string();
    if actual_hash != provenance.effective_config_hash {
        return Err(miette::miette!(
            "recorded Ghostty profile hash does not match {}",
            path.display()
        ));
    }
    return Ok(ResolvedGhosttyProfile {
        effective_config,
        provenance: provenance.clone(),
    });
}

fn run_playback(arguments: PlaybackArgs) -> Result<ExitCode> {
    playback(&PlaybackOptions {
        session_directory: arguments.session,
        timing_file: arguments.timing_file,
        error_file: arguments.error_file,
        ready_file: arguments.ready_file,
        start_delay: Duration::from_millis(arguments.start_delay_ms),
        final_hold: Duration::from_millis(arguments.final_hold_ms),
        start_monotonic_ns: arguments.from_ns,
        end_monotonic_ns: arguments.to_ns,
        endpoint_event_seq: arguments.endpoint_event_seq,
        endpoint_event_byte_offset: arguments.endpoint_event_byte_offset,
        instant: arguments.instant,
    })?;
    return Ok(ExitCode::SUCCESS);
}

fn run_inspect(arguments: InspectArgs) -> Result<ExitCode> {
    let selector = match (arguments.state, arguments.event_seq, arguments.monotonic_ns) {
        (Some(state), None, None) => StateSelector::StateId(state),
        (None, Some(sequence), None) => StateSelector::EventSeq(sequence),
        (None, None, Some(time)) => StateSelector::MonotonicNs(time),
        (None, None, None) => StateSelector::Final,
        _ => {
            return Err(miette::miette!(
                "exactly one state selector may be provided"
            ));
        }
    };
    let inspection = inspect_session(arguments.session, selector)?;
    if let Some(row_number) = arguments.row {
        let row = inspection
            .state
            .grid
            .iter()
            .find(|row| return row.row == row_number)
            .ok_or_else(|| miette::miette!("semantic row {row_number} does not exist"))?;
        let record = serde_json::json!({
            "state_id": inspection.record.state_id,
            "event_seq": inspection.record.event_seq,
            "event_byte_offset": inspection.record.event_byte_offset,
            "monotonic_ns": inspection.record.monotonic_ns,
            "state_hash": inspection.record.state_hash,
            "presented": inspection.record.presented,
        });
        return match arguments.format {
            InspectFormat::Json => write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({
                    "record": record,
                    "row": row,
                }))
                .into_diagnostic()?
            )),
            InspectFormat::JsonCompact => write_json_line(&serde_json::json!({
                "record": record,
                "row": row,
            })),
            InspectFormat::Text => write_stdout(format!("{}\n", row_text(row))),
        };
    }
    match arguments.format {
        InspectFormat::Json => {
            return write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&inspection).into_diagnostic()?
            ));
        }
        InspectFormat::JsonCompact => return write_json_line(&inspection),
        InspectFormat::Text => {
            return write_stdout(format!("{}\n", state_text(&inspection.state)));
        }
    }
}

fn run_show(arguments: ShowArgs) -> Result<ExitCode> {
    let image_format = matches!(arguments.format, ShowFormat::Svg | ShowFormat::Png);
    if !image_format && arguments.renderer == ShowRenderer::Native {
        return Err(miette::miette!(
            "--renderer applies only to SVG and PNG show formats"
        ));
    }
    if arguments.renderer == ShowRenderer::Native && arguments.format != ShowFormat::Png {
        return Err(miette::miette!(
            "native show output is PNG; semantic SVG fallback is disabled"
        ));
    }
    let point = match (
        arguments.state,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
    ) {
        (Some(state_id), None, None, None) => StatePoint::StateId { state_id },
        (None, Some(event_seq), None, None) => StatePoint::EventSeq { event_seq },
        (None, None, Some(monotonic_ns), None) => StatePoint::MonotonicNs { monotonic_ns },
        (None, None, None, Some(label)) => StatePoint::Marker { label },
        (None, None, None, None) => StatePoint::LatestPresented,
        _ => return Err(miette::miette!("exactly one show selector may be supplied")),
    };
    let one_off_wait = match (&arguments.wait, &arguments.wait_regex) {
        (Some(text), None) => Some(ScreenPredicate::Contains { text: text.clone() }),
        (None, Some(pattern)) => Some(ScreenPredicate::MatchesRegex {
            pattern: pattern.clone(),
        }),
        (None, None) => None,
        _ => return Err(miette::miette!("exactly one one-off wait may be supplied")),
    };
    if arguments.record_out.is_some() && arguments.input.is_none() {
        return Err(miette::miette!("--record-out requires --input"));
    }
    if (arguments.out.is_some() || arguments.discard_evidence) && arguments.command.is_empty() {
        return Err(miette::miette!(
            "--out and --discard-evidence require a one-off command after --"
        ));
    }
    if arguments.discard_evidence && arguments.detach {
        return Err(miette::miette!(
            "--discard-evidence cannot remove a detached live one-off command"
        ));
    }
    if (one_off_wait.is_some() || arguments.detach) && arguments.command.is_empty() {
        return Err(miette::miette!(
            "--wait, --wait-regex, and --detach require a one-off command after --"
        ));
    }
    if arguments.detach {
        if arguments.session.is_some()
            || arguments.input.is_some()
            || point != StatePoint::LatestPresented
            || one_off_wait.is_some()
        {
            return Err(miette::miette!(
                "detached one-off show accepts only geometry and a command"
            ));
        }
        let outcome = run_one_off_command(
            &arguments.command,
            arguments.cols,
            arguments.rows,
            None,
            Duration::ZERO,
            Duration::from_millis(arguments.timeout_ms),
            true,
            arguments.out.as_deref(),
        )?;
        let source_completed = outcome
            .observation
            .as_ref()
            .is_some_and(|observation| return observation.source_completed);
        return write_json_line(&serde_json::json!({
            "kind": "detached_one_off",
            "canonical_session": ByteString::from(outcome.session.as_os_str().as_bytes()),
            "detached": outcome.detached,
            "source_completed": source_completed,
        }));
    }
    if arguments.recover_prefix {
        let session = arguments.session.as_ref().ok_or_else(|| {
            return miette::miette!("--recover-prefix requires a session source");
        })?;
        if arguments.input.is_some()
            || !arguments.command.is_empty()
            || !arguments.stable_for.is_zero()
            || image_format
            || matches!(arguments.format, ShowFormat::Vt | ShowFormat::Html)
            || arguments.manifest.is_some()
        {
            return Err(miette::miette!(
                "prefix recovery supports session text, JSON, compact JSON, or frame JSON without live stability"
            ));
        }
        let session = resolve_session_ref(session.clone())?;
        let observation = observe_validated_prefix(&session, point, arguments.include_hidden)?;
        if arguments.metadata {
            writeln!(
                io::stderr(),
                "source={:?} resolution={:?} state_hash={} presented={} quality={:?} truth={:?}",
                observation.source_kind,
                observation.resolution,
                observation.state_hash,
                observation.presented,
                observation.capture_quality,
                observation.truth_layer,
            )
            .into_diagnostic()?;
        }
        return match arguments.format {
            ShowFormat::Text => write_stdout(format!("{}\n", observation.frame.viewport_text)),
            ShowFormat::Json => write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&observation).into_diagnostic()?
            )),
            ShowFormat::JsonCompact => write_json_line(&observation),
            ShowFormat::FrameJson => write_json_line(&observation.frame_envelope()),
            _ => Err(miette::miette!("unsupported prefix observation format")),
        };
    }
    let formatter = match arguments.format {
        ShowFormat::Vt => Some(GhosttyFormat::Vt),
        ShowFormat::Html => Some(GhosttyFormat::Html),
        _ => None,
    };
    let (observation, formatted, image, discard_session) = match (
        arguments.session,
        arguments.input,
        arguments.command.is_empty(),
    ) {
        (Some(session), None, true) => {
            let session = resolve_session_ref(session)?;
            let mut stable_hash = None;
            if !arguments.stable_for.is_zero() && !session.join("completion.json").is_file() {
                if point != StatePoint::LatestPresented || arguments.include_hidden {
                    return Err(miette::miette!(
                        "live stability is valid only for latest-presented observations"
                    ));
                }
                stable_hash = Some(
                    wait_for_screen(
                        &session,
                        ScreenPredicate::StateHash {
                            hash: "0".repeat(64),
                            comparison: StateHashComparison::Changed,
                        },
                        arguments.stable_for,
                        Duration::from_millis(arguments.timeout_ms),
                    )?
                    .state_hash,
                );
            }
            let mut observation =
                observe_session_with_options(&session, point.clone(), arguments.include_hidden)?;
            if let Some(state_hash) = stable_hash {
                if observation.state_hash != state_hash {
                    return Err(miette::miette!(
                        "presented state changed between stability proof and observation; retry show"
                    ));
                }
                observation.capture_quality = CaptureQuality::Stable;
            }
            let formatted = match formatter {
                Some(format) => {
                    if !session.join("completion.json").is_file() {
                        return Err(miette::miette!(
                            "live VT/HTML formatting requires completed evidence"
                        ));
                    }
                    Some(format_completed_state(&session, &point, format)?)
                }
                None => None,
            };
            let image = if image_format && arguments.renderer == ShowRenderer::Native {
                observation.truth_layer = TruthLayer::Native;
                Some(render_native_show(&session, &point)?)
            } else if image_format {
                let state = semantic_state_for_session(&session, &point, arguments.include_hidden)?;
                ensure_observation_state(&observation, &state)?;
                Some(render_show_image(&state, arguments.format)?)
            } else {
                None
            };
            (observation, formatted, image, None)
        }
        (None, Some(input), true) => {
            if arguments.renderer == ShowRenderer::Native {
                return Err(miette::miette!(
                    "native show rendering requires canonical session evidence; semantic fallback is disabled"
                ));
            }
            if !arguments.stable_for.is_zero() {
                return Err(miette::miette!(
                    "--stable-for requires a live session source"
                ));
            }
            if point != StatePoint::LatestPresented || arguments.include_hidden {
                return Err(miette::miette!(
                    "ANSI streams support only latest-presented selection"
                ));
            }
            let bytes = if input == Path::new("-") {
                let mut bytes = Vec::new();
                io::stdin().read_to_end(&mut bytes).into_diagnostic()?;
                bytes
            } else {
                fs_err::read(&input).into_diagnostic()?
            };
            if let Some(destination) = &arguments.record_out {
                record_ansi_import(&bytes, &input, destination, arguments.cols, arguments.rows)?;
                let mut observation =
                    observe_one_off_session(destination, StatePoint::LatestPresented, false)?;
                observation.source_kind = ObservationSourceKind::ImportedAnsi;
                let formatted = formatter
                    .map(|format| {
                        return format_completed_state(
                            destination,
                            &StatePoint::LatestPresented,
                            format,
                        );
                    })
                    .transpose()?;
                let image = if image_format {
                    let state = semantic_state_for_session(
                        destination,
                        &StatePoint::LatestPresented,
                        false,
                    )?;
                    ensure_observation_state(&observation, &state)?;
                    Some(render_show_image(&state, arguments.format)?)
                } else {
                    None
                };
                (observation, formatted, image, None)
            } else {
                let formatted = formatter
                    .map(|format| {
                        return format_ansi_stream(&bytes, arguments.cols, arguments.rows, format);
                    })
                    .transpose()?;
                let observation = observe_ansi_stream(&bytes, arguments.cols, arguments.rows)?;
                let image = if image_format {
                    let state = semantic_state_from_ansi(&bytes, arguments.cols, arguments.rows)?;
                    ensure_observation_state(&observation, &state)?;
                    Some(render_show_image(&state, arguments.format)?)
                } else {
                    None
                };
                (observation, formatted, image, None)
            }
        }
        (None, None, false) => {
            if !arguments.stable_for.is_zero() && one_off_wait.is_none() {
                return Err(miette::miette!(
                    "one-off --stable-for requires --wait or --wait-regex"
                ));
            }
            if one_off_wait.is_some()
                && (point != StatePoint::LatestPresented
                    || arguments.include_hidden
                    || formatter.is_some()
                    || image_format)
            {
                return Err(miette::miette!(
                    "one-off waits return their exact compact/text/JSON observation and do not accept selectors, hidden states, or rendered formats"
                ));
            }
            let outcome = run_one_off_command(
                &arguments.command,
                arguments.cols,
                arguments.rows,
                one_off_wait,
                arguments.stable_for,
                Duration::from_millis(arguments.timeout_ms),
                false,
                arguments.out.as_deref(),
            )?;
            let diagnostic_logs = outcome.diagnostic_logs;
            let session = outcome.session;
            let mut observation = match outcome.observation {
                Some(observation) => observation,
                None => observe_one_off_session(&session, point.clone(), arguments.include_hidden)?,
            };
            let formatted = formatter
                .map(|format| return format_completed_state(&session, &point, format))
                .transpose()?;
            let image = if image_format && arguments.renderer == ShowRenderer::Native {
                observation.truth_layer = TruthLayer::Native;
                Some(render_native_show(&session, &point)?)
            } else if image_format {
                let state = semantic_state_for_session(&session, &point, arguments.include_hidden)?;
                ensure_observation_state(&observation, &state)?;
                Some(render_show_image(&state, arguments.format)?)
            } else {
                None
            };
            let discard_session = arguments
                .discard_evidence
                .then_some((session, diagnostic_logs));
            (observation, formatted, image, discard_session)
        }
        _ => return Err(miette::miette!("exactly one show source is required")),
    };
    if arguments.metadata {
        writeln!(
            io::stderr(),
            "source={:?} resolution={:?} state_hash={} presented={} quality={:?} truth={:?}",
            observation.source_kind,
            observation.resolution,
            observation.state_hash,
            observation.presented,
            observation.capture_quality,
            observation.truth_layer,
        )
        .into_diagnostic()?;
    }
    if let (Some(path), Some(bytes)) = (&arguments.manifest, &image) {
        let renderer = match (arguments.format, arguments.renderer) {
            (ShowFormat::Svg, ShowRenderer::Semantic) => SEMANTIC_SVG_RENDERER,
            (ShowFormat::Png, ShowRenderer::Semantic) => SEMANTIC_PNG_RENDERER,
            (ShowFormat::Png, ShowRenderer::Native) => "ghostty-native-post-endpoint-v1",
            _ => return Err(miette::miette!("show manifest requires an image format")),
        };
        let manifest = serde_json::to_vec_pretty(&serde_json::json!({
            "format": "termtrace-show-image",
            "version": 1,
            "renderer": renderer,
            "truth_layer": if arguments.renderer == ShowRenderer::Native { "native" } else { "semantic_rendering" },
            "media_type": if matches!(arguments.format, ShowFormat::Svg) { "image/svg+xml" } else { "image/png" },
            "byte_count": bytes.len(),
            "blake3": blake3::hash(bytes).to_hex().to_string(),
            "observation": observation,
        }))
        .into_diagnostic()?;
        write_secure(path, &manifest)?;
    } else if arguments.manifest.is_some() {
        return Err(miette::miette!(
            "--manifest is valid only for SVG or PNG output"
        ));
    }
    let result = match arguments.format {
        ShowFormat::Text => write_stdout(format!("{}\n", observation.frame.viewport_text)),
        ShowFormat::Json => write_stdout(format!(
            "{}\n",
            serde_json::to_string_pretty(&observation).into_diagnostic()?
        )),
        ShowFormat::JsonCompact => write_json_line(&observation),
        ShowFormat::FrameJson => write_json_line(&observation.frame_envelope()),
        ShowFormat::Vt | ShowFormat::Html => write_stdout(formatted.ok_or_else(|| {
            return miette::miette!("selected formatter produced no output");
        })?),
        ShowFormat::Svg | ShowFormat::Png => write_stdout(image.ok_or_else(|| {
            return miette::miette!("selected image renderer produced no output");
        })?),
    };
    if result.is_ok()
        && let Some((session, diagnostic_logs)) = discard_session
    {
        fs_err::remove_dir_all(&session).into_diagnostic()?;
        for log in diagnostic_logs {
            if log.exists() {
                fs_err::remove_file(log).into_diagnostic()?;
            }
        }
        writeln!(
            io::stderr(),
            "termtrace: discarded one-off evidence {}; the returned observation is no longer independently replayable",
            session.display()
        )
        .into_diagnostic()?;
    }
    return result;
}

fn render_show_image(state: &SemanticState, format: ShowFormat) -> Result<Vec<u8>> {
    return match format {
        ShowFormat::Svg => Ok(render_svg(state)),
        ShowFormat::Png => render_png(state).into_diagnostic(),
        _ => Err(miette::miette!("show image renderer requires SVG or PNG")),
    };
}

fn ensure_observation_state(observation: &Observation, state: &SemanticState) -> Result<()> {
    let bytes = serde_json::to_vec(state).into_diagnostic()?;
    let state_hash = blake3::hash(&bytes).to_hex().to_string();
    if observation.state_hash != state_hash {
        return Err(miette::miette!(
            "semantic image state changed after observation; retry show"
        ));
    }
    return Ok(());
}

fn render_native_show(session: &Path, point: &StatePoint) -> Result<Vec<u8>> {
    if !session.join("completion.json").is_file() {
        return Err(miette::miette!(
            "native show rendering requires completed canonical evidence"
        ));
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let inspection = inspect_point(&log, point)?;
    if !inspection.record.presented {
        return Err(miette::miette!(
            "native show rendering rejects hidden semantic states"
        ));
    }
    let event_seq = inspection
        .record
        .event_seq
        .ok_or_else(|| return miette::miette!("the initial state has no native replay endpoint"))?;
    let (target, geometry_solver) = native_capture_target(session, None)?;
    let destination =
        env::temp_dir().join(format!(".termtrace-native-show-{}", uuid::Uuid::now_v7()));
    let outcome = capture(&CaptureOptions {
        session_directory: session.to_path_buf(),
        width: target.width,
        height: target.height,
        fps: 24,
        start_monotonic_ns: None,
        end_monotonic_ns: None,
        state_selection: StateSelection::Presented,
        geometry_solver,
        kind: CaptureKind::Screenshot {
            state_id: inspection.record.state_id,
            state_hash: inspection.record.state_hash,
            event_seq,
            event_byte_offset: inspection.record.event_byte_offset,
        },
        destination: Some(destination.clone()),
    })?;
    let image = fs_err::read(outcome.screenshot).into_diagnostic()?;
    fs_err::remove_dir_all(&destination)
        .map_err(|error| return miette::miette!("remove native show staging artifact: {error}"))?;
    return Ok(image);
}

fn format_completed_state(
    session: &Path,
    point: &StatePoint,
    format: GhosttyFormat,
) -> Result<Vec<u8>> {
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let inspection = inspect_point(&log, point)?;
    return Ok(format_state(
        &log,
        StateSelector::StateId(inspection.record.state_id),
        format,
    )?);
}

fn run_timeline(arguments: TimelineArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )?;
    let entries = build_timeline(&log, arguments.all_states, arguments.controls, range)?;
    if arguments.jsonl {
        for entry in entries {
            write_json_line(&entry)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut output = String::new();
    for entry in entries {
        writeln!(output, "{}", timeline_text(&entry)).into_diagnostic()?;
    }
    return write_stdout(output);
}

fn run_diff(arguments: DiffArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let from = diff_point(
        arguments.from_state,
        arguments.from_event_seq,
        arguments.from_at,
        arguments.from_marker,
    )?;
    let to = diff_point(
        arguments.to_state,
        arguments.to_event_seq,
        arguments.to_at,
        arguments.to_marker,
    )?;
    let image_states = if matches!(arguments.format, DiffFormat::Svg | DiffFormat::Png) {
        Some((
            inspect_point(&log, &from)?.state,
            inspect_point(&log, &to)?.state,
        ))
    } else {
        None
    };
    let difference = diff_states(&log, from, to)?;
    let changed = difference.changed;
    let _output_status = match arguments.format {
        DiffFormat::Text => write_stdout(diff_text(&difference)?)?,
        DiffFormat::Json => write_stdout(format!(
            "{}\n",
            serde_json::to_string_pretty(&difference).into_diagnostic()?
        ))?,
        DiffFormat::JsonCompact => write_json_line(&difference)?,
        DiffFormat::Svg => {
            let (before, after) = image_states
                .as_ref()
                .ok_or_else(|| return miette::miette!("missing visual diff states"))?;
            write_stdout(render_diff_svg(before, after, &difference))?
        }
        DiffFormat::Png => {
            let (before, after) = image_states
                .as_ref()
                .ok_or_else(|| return miette::miette!("missing visual diff states"))?;
            write_stdout(render_diff_png(before, after, &difference)?)?
        }
    };
    if arguments.expect_no_change && changed {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_explain(arguments: ExplainArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let target = diff_point(
        arguments.state,
        arguments.event_seq,
        arguments.at,
        arguments.marker,
    )?;
    let report = explain(&log, target, arguments.context)?;
    return match arguments.format {
        ExplainFormat::Text => write_stdout(explain_text(&report)?),
        ExplainFormat::Json => write_stdout(format!(
            "{}\n",
            serde_json::to_string_pretty(&report).into_diagnostic()?
        )),
        ExplainFormat::JsonCompact => write_json_line(&report),
    };
}

fn run_diagnose(arguments: DiagnoseArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let report = diagnose(
        &session,
        &log,
        arguments.check,
        arguments.broad_change_percent,
    )?;
    let has_findings = !report.findings.is_empty();
    if arguments.json {
        write_json_line(&report)?;
    } else {
        write_stdout(diagnose_text(&report)?)?;
    }
    if arguments.fail_on_finding && has_findings {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_output(arguments: OutputArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if arguments.follow {
        if arguments.manifest.is_some() {
            return Err(miette::miette!(
                "--manifest is available only for completed non-follow output"
            ));
        }
        if arguments.range.from_marker.is_some()
            || arguments.range.to_marker.is_some()
            || arguments.range.start_monotonic_ns.is_some()
            || arguments.range.end_monotonic_ns.is_some()
        {
            return Err(miette::miette!(
                "live output following currently supports event-sequence bounds only"
            ));
        }
        return follow_output(
            &session,
            arguments.from_event_seq.unwrap_or(0),
            arguments.to_event_seq,
            arguments.strip_controls,
        );
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    let completion = validate_completed_log(&session, &log)?;
    let time_range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )?;
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "output event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }
    let mut output = Vec::new();
    for record in &log.events {
        if record.seq < from_event
            || record.seq >= to_event
            || time_range.is_some_and(|range| return !range.contains(record.monotonic_ns))
        {
            continue;
        }
        if let Event::Output { data } = &record.event {
            output.extend_from_slice(data.as_bytes());
        }
    }
    let source = output;
    let (output, quality, dropped, alternate_screen) = if arguments.strip_controls {
        let mut stripper = ControlStripper::default();
        let output = stripper.feed(&source);
        let alternate_screen = stripper.alternate_screen_observed();
        let quality = if alternate_screen {
            OutputProjectionQuality::UnsuitableForFullScreen
        } else {
            OutputProjectionQuality::LossyLineOriented
        };
        (
            output,
            quality,
            stripper
                .source_byte_count()
                .saturating_sub(stripper.output_byte_count()),
            alternate_screen,
        )
    } else {
        (
            source.clone(),
            OutputProjectionQuality::ExactBytes,
            0,
            false,
        )
    };
    let warning = projection_warning(quality);
    if let Some(path) = &arguments.manifest {
        let report = OutputProjectionReport {
            format: "termtrace-output-projection".to_owned(),
            version: 1,
            session_id: log.header.session_id,
            final_event_hash: completion.final_event_hash,
            from_event_seq: from_event,
            to_event_seq: to_event,
            mode: if arguments.raw {
                "raw"
            } else {
                "strip_controls"
            }
            .to_owned(),
            projection_quality: quality,
            source_byte_count: u64::try_from(source.len()).unwrap_or(u64::MAX),
            source_blake3: blake3::hash(&source).to_hex().to_string(),
            output_byte_count: u64::try_from(output.len()).unwrap_or(u64::MAX),
            output_blake3: blake3::hash(&output).to_hex().to_string(),
            dropped_byte_count: dropped,
            alternate_screen_observed: alternate_screen,
            warning: warning.map(str::to_owned),
        };
        let mut bytes = serde_json::to_vec_pretty(&report).into_diagnostic()?;
        bytes.push(b'\n');
        write_secure(path, &bytes)?;
    }
    if arguments.strip_controls {
        write_projection_warning(warning.unwrap_or("control-stripped output is lossy"))?;
    }
    return write_stdout(output);
}

fn follow_output(
    session: &Path,
    mut next_event_seq: u64,
    end_event_seq: Option<u64>,
    strip_controls: bool,
) -> Result<ExitCode> {
    let mut control_failure_since = None;
    let mut stripper = ControlStripper::default();
    loop {
        let completion_exists = session.join("completion.json").is_file();
        let log = if completion_exists {
            let log = EventLog::read(session.join("events.jsonl"))?;
            validate_completed_log(session, &log)?;
            log
        } else {
            let response = match request(session, ControlCommand::Status) {
                Ok(response) => {
                    control_failure_since = None;
                    response
                }
                Err(_) if session.join("completion.json").is_file() => continue,
                Err(error) => {
                    let since = control_failure_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(2) {
                        return Err(error.into());
                    }
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            let ControlData::Status { boundaries, .. } = response else {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            };
            EventLog::read_prefix(
                session.join("events.jsonl"),
                boundaries.durable.next_event_seq,
            )?
        };
        let available = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
        if next_event_seq > available {
            return Err(miette::miette!(
                "output resume cursor {next_event_seq} exceeds durable event count {available}"
            ));
        }
        let emit_end = end_event_seq.unwrap_or(available).min(available);
        let mut bytes = Vec::new();
        for record in &log.events {
            if record.seq < next_event_seq || record.seq >= emit_end {
                continue;
            }
            if let Event::Output { data } = &record.event {
                bytes.extend_from_slice(data.as_bytes());
            }
        }
        let emitted = if strip_controls {
            stripper.feed(&bytes)
        } else {
            bytes
        };
        if !emitted.is_empty() && !write_follow_stdout(&emitted)? {
            return Ok(ExitCode::SUCCESS);
        }
        next_event_seq = emit_end;
        if completion_exists && end_event_seq.is_some_and(|end| return end > available) {
            return Err(miette::miette!(
                "output end cursor exceeds completed event count {available}"
            ));
        }
        if end_event_seq.is_some_and(|end| return next_event_seq >= end)
            || (completion_exists && next_event_seq >= available)
        {
            if strip_controls {
                let quality = if stripper.alternate_screen_observed() {
                    OutputProjectionQuality::UnsuitableForFullScreen
                } else {
                    OutputProjectionQuality::LossyLineOriented
                };
                write_projection_warning(
                    projection_warning(quality).unwrap_or("control-stripped output is lossy"),
                )?;
            }
            return Ok(ExitCode::SUCCESS);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn projection_warning(quality: OutputProjectionQuality) -> Option<&'static str> {
    return match quality {
        OutputProjectionQuality::ExactBytes => None,
        OutputProjectionQuality::LossyLineOriented => Some(
            "projection_quality=lossy_line_oriented; controls were removed; use show/timeline for viewport state",
        ),
        OutputProjectionQuality::UnsuitableForFullScreen => Some(
            "projection_quality=unsuitable_for_full_screen; alternate-screen controls were observed; use show/timeline for viewport state",
        ),
    };
}

fn write_projection_warning(message: &str) -> Result<()> {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "termtrace: {message}").into_diagnostic()?;
    return Ok(());
}

fn write_follow_stdout(output: &[u8]) -> Result<bool> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    return match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error).into_diagnostic(),
    };
}

fn write_follow_json_line(value: &impl serde::Serialize) -> Result<bool> {
    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');
    return write_follow_stdout(&encoded);
}

fn run_events(arguments: EventsArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if arguments.follow {
        if arguments.range.from_marker.is_some()
            || arguments.range.to_marker.is_some()
            || arguments.range.start_monotonic_ns.is_some()
            || arguments.range.end_monotonic_ns.is_some()
        {
            return Err(miette::miette!(
                "live event following currently supports event-sequence bounds only"
            ));
        }
        return follow_events(
            &session,
            arguments.from_event_seq.unwrap_or(0),
            arguments.to_event_seq,
            &arguments.event_types,
            arguments.metadata_only,
        );
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let time_range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )?;
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }
    for record in &log.events {
        let kind = event_type(&record.event);
        if record.seq < from_event
            || record.seq >= to_event
            || time_range.is_some_and(|range| return !range.contains(record.monotonic_ns))
            || (!arguments.event_types.is_empty()
                && !arguments
                    .event_types
                    .iter()
                    .any(|value| return value == kind))
        {
            continue;
        }
        write_json_line(&EventProjection::from_record(
            record,
            arguments.metadata_only,
        ))?;
    }
    return Ok(ExitCode::SUCCESS);
}

fn follow_events(
    session: &Path,
    mut next_event_seq: u64,
    end_event_seq: Option<u64>,
    event_types: &[String],
    metadata_only: bool,
) -> Result<ExitCode> {
    let mut control_failure_since = None;
    loop {
        let completion_exists = session.join("completion.json").is_file();
        let log = if completion_exists {
            let log = EventLog::read(session.join("events.jsonl"))?;
            validate_completed_log(session, &log)?;
            log
        } else {
            let response = match request(session, ControlCommand::Status) {
                Ok(response) => {
                    control_failure_since = None;
                    response
                }
                Err(_) if session.join("completion.json").is_file() => continue,
                Err(error) => {
                    let since = control_failure_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(2) {
                        return Err(error.into());
                    }
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            let ControlData::Status { boundaries, .. } = response else {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            };
            EventLog::read_prefix(
                session.join("events.jsonl"),
                boundaries.durable.next_event_seq,
            )?
        };
        let available = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
        if next_event_seq > available {
            return Err(miette::miette!(
                "event resume cursor {next_event_seq} exceeds durable event count {available}"
            ));
        }
        let emit_end = end_event_seq.unwrap_or(available).min(available);
        for record in &log.events {
            let kind = event_type(&record.event);
            if record.seq < next_event_seq
                || record.seq >= emit_end
                || (!event_types.is_empty()
                    && !event_types.iter().any(|value| return value == kind))
            {
                continue;
            }
            if !write_follow_json_line(&EventProjection::from_record(record, metadata_only))? {
                return Ok(ExitCode::SUCCESS);
            }
        }
        next_event_seq = emit_end;
        if completion_exists && end_event_seq.is_some_and(|end| return end > available) {
            return Err(miette::miette!(
                "event end cursor exceeds completed event count {available}"
            ));
        }
        if end_event_seq.is_some_and(|end| return next_event_seq >= end)
            || (completion_exists && next_event_seq >= available)
        {
            return Ok(ExitCode::SUCCESS);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn run_completions(arguments: CompletionsArgs) -> Result<ExitCode> {
    let mut command = Cli::command();
    generate(
        arguments.shell,
        &mut command,
        "termtrace",
        &mut io::stdout(),
    );
    return Ok(ExitCode::SUCCESS);
}

fn run_driver() -> Result<ExitCode> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve_driver(BufReader::new(stdin.lock()), stdout.lock())?;
    return Ok(ExitCode::SUCCESS);
}

fn run_save(arguments: SaveArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let point = match (
        arguments.state,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
    ) {
        (Some(state_id), None, None, None) => StatePoint::StateId { state_id },
        (None, Some(event_seq), None, None) => StatePoint::EventSeq { event_seq },
        (None, None, Some(monotonic_ns), None) => StatePoint::MonotonicNs { monotonic_ns },
        (None, None, None, Some(label)) => StatePoint::Marker { label },
        (None, None, None, None) => StatePoint::LatestPresented,
        _ => return Err(miette::miette!("exactly one save selector may be supplied")),
    };
    let formats = arguments
        .format
        .into_iter()
        .map(SaveFormat::from)
        .collect::<Vec<_>>();
    let outcome = save(
        &session,
        &arguments.out,
        point,
        &formats,
        arguments.include_hidden,
    )?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "state_id": outcome.manifest.state_id,
        "state_hash": outcome.manifest.state_hash,
        "files": outcome.manifest.files,
        "manifest": outcome.directory.join("save-manifest.json"),
    }));
}

fn run_screenshot(arguments: ScreenshotArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let mut points = Vec::new();
    points.extend(
        arguments
            .state
            .into_iter()
            .map(|state_id| return StatePoint::StateId { state_id }),
    );
    points.extend(
        arguments
            .event_seq
            .into_iter()
            .map(|event_seq| return StatePoint::EventSeq { event_seq }),
    );
    points.extend(
        arguments
            .at
            .into_iter()
            .map(|monotonic_ns| return StatePoint::MonotonicNs { monotonic_ns }),
    );
    points.extend(
        arguments
            .at_marker
            .into_iter()
            .map(|label| return StatePoint::Marker { label }),
    );
    if points.is_empty() {
        points.push(StatePoint::LatestPresented);
    }
    if arguments.renderer == ShowRenderer::Native {
        if arguments.format != ScreenshotFormat::Png {
            return Err(miette::miette!(
                "native screenshots are PNG; semantic SVG fallback is disabled"
            ));
        }
        if arguments.include_hidden {
            return Err(miette::miette!(
                "native screenshots cannot render hidden synchronized-output states"
            ));
        }
        if points.len() != 1 {
            return Err(miette::miette!(
                "native screenshot batches are not yet a single-process operation; select one state"
            ));
        }
        let point = points
            .first()
            .ok_or_else(|| return miette::miette!("missing native screenshot point"))?;
        let log = EventLog::read(session.join("events.jsonl"))?;
        validate_completed_log(&session, &log)?;
        let inspection = inspect_point(&log, point)?;
        if !inspection.record.presented {
            return Err(miette::miette!(
                "native screenshots reject states that were not presentation eligible"
            ));
        }
        let event_seq = inspection.record.event_seq.ok_or_else(|| {
            return miette::miette!("the initial blank state has no native replay endpoint");
        })?;
        let (target, geometry_solver) = native_capture_target(&session, arguments.viewport)?;
        let outcome = capture(&CaptureOptions {
            session_directory: session,
            width: target.width,
            height: target.height,
            fps: 24,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::Presented,
            geometry_solver,
            kind: CaptureKind::Screenshot {
                state_id: inspection.record.state_id,
                state_hash: inspection.record.state_hash,
                event_seq,
                event_byte_offset: inspection.record.event_byte_offset,
            },
            destination: Some(arguments.out),
        })?;
        return write_json_line(&serde_json::json!({
            "kind": "native_screenshot",
            "truth_layer": "native",
            "directory": outcome.directory,
            "image": outcome.screenshot,
            "manifest": outcome.directory.join("manifest.json"),
            "presentation_evidence": "post_endpoint_stable_capture",
        }));
    }
    if arguments.viewport.is_some() || arguments.fit_viewport || arguments.strict {
        return Err(miette::miette!(
            "native viewport options require --renderer native"
        ));
    }
    let format = match arguments.format {
        ScreenshotFormat::Svg => SaveFormat::Svg,
        ScreenshotFormat::Png => SaveFormat::Png,
    };
    let filename = match arguments.format {
        ScreenshotFormat::Svg => "screen.svg",
        ScreenshotFormat::Png => "screen.png",
    };
    if points.len() > 1 {
        let manifest = save_screenshot_batch(
            &session,
            &arguments.out,
            &points,
            format,
            arguments.include_hidden,
        )?;
        return write_json_line(&serde_json::json!({
            "kind": "semantic_screenshot_batch",
            "truth_layer": "semantic_rendering",
            "directory": arguments.out,
            "manifest": arguments.out.join("batch-manifest.json"),
            "entries": manifest.entries,
        }));
    }
    let point = points
        .into_iter()
        .next()
        .ok_or_else(|| return miette::miette!("missing screenshot point"))?;
    let outcome = save(
        &session,
        &arguments.out,
        point,
        &[format],
        arguments.include_hidden,
    )?;
    return write_json_line(&serde_json::json!({
        "kind": "semantic_screenshot",
        "truth_layer": "semantic_rendering",
        "directory": outcome.directory,
        "image": outcome.directory.join(filename),
        "manifest": outcome.directory.join("save-manifest.json"),
        "state_hash": outcome.manifest.state_hash,
    }));
}

fn native_capture_target(
    session: &Path,
    viewport: Option<CaptureTarget>,
) -> Result<(CaptureTarget, Option<GeometrySolveReport>)> {
    if let Some(target) = viewport {
        return Ok((target, None));
    }
    let manifest = read_manifest(session)?;
    let initial = manifest
        .geometry_provenance
        .capture_target
        .unwrap_or_else(|| {
            return CaptureTarget {
                width: manifest.geometry.pixel_width.saturating_add(128).max(320),
                height: manifest.geometry.pixel_height.saturating_add(128).max(200),
            };
        });
    let profile = capture_profile(session, &manifest)?;
    let report = solve_ghostty_geometry(
        manifest.geometry.cols,
        manifest.geometry.rows,
        initial,
        &profile,
    )?;
    return Ok((report.selected, Some(report)));
}

fn run_bundle(arguments: BundleArgs) -> Result<ExitCode> {
    let profile = arguments.profile.into();
    if arguments.dry_run {
        return write_json_line(&plan_bundle(profile));
    }
    let session = resolve_session_ref(arguments.session)?;
    if arguments.archive {
        if arguments.out.exists() {
            return Err(Error::ArtifactExists {
                path: arguments.out,
            }
            .into());
        }
        let parent = arguments
            .out
            .parent()
            .ok_or_else(|| return miette::miette!("bundle archive path has no parent"))?;
        let staging = parent.join(format!(".bundle-source-{}.tmp", uuid::Uuid::now_v7()));
        let manifest = create_bundle(&session, &staging, profile)?;
        let result = archive_bundle(&staging, &arguments.out);
        let _cleanup = fs_err::remove_dir_all(&staging);
        result?;
        return write_json_line(&serde_json::json!({
            "kind": "bundle_archive",
            "profile": manifest.profile,
            "archive": arguments.out,
            "member_count": manifest.members.len(),
            "includes_canonical_evidence": manifest.includes_canonical_evidence,
        }));
    }
    let manifest = create_bundle(&session, &arguments.out, profile)?;
    return write_json_line(&serde_json::json!({
        "kind": "bundle",
        "directory": arguments.out,
        "manifest": manifest,
    }));
}

fn run_bundle_verify(arguments: BundleVerifyArgs) -> Result<ExitCode> {
    let manifest = if arguments.bundle.is_dir() {
        verify_bundle(&arguments.bundle)?
    } else {
        verify_bundle_archive(&arguments.bundle)?
    };
    return write_json_line(&serde_json::json!({
        "kind": "bundle_verification",
        "valid": true,
        "bundle": arguments.bundle,
        "manifest": manifest,
    }));
}

fn explain_text(report: &ExplainReport) -> Result<String> {
    let mut output = format!(
        "state {} event {:?} context={} tokens={} states={}\n",
        report.target_state.state_id,
        report.target_state.event_seq,
        report.context_events,
        report.tokens.len(),
        report.states.len(),
    );
    for token in &report.tokens {
        let source = token.source.first().map_or_else(
            || return "event - bytes -".to_owned(),
            |segment| {
                return format!(
                    "event {} bytes {}..{}",
                    segment.event_seq, segment.start_byte_offset, segment.end_byte_offset
                );
            },
        );
        writeln!(
            output,
            "  token {} {source} {:?}: {}",
            token.token_id, token.kind, token.description
        )
        .into_diagnostic()?;
    }
    for finding in &report.findings {
        writeln!(
            output,
            "finding: {} [{:?}] {}",
            finding.code,
            finding.confidence,
            finding.facts.join("; ")
        )
        .into_diagnostic()?;
    }
    return Ok(output);
}

fn diagnose_text(report: &DiagnoseReport) -> Result<String> {
    let mut output = format!(
        "session {} checks={} findings={} source={}\n",
        report.session_id,
        report.checks.len(),
        report.findings.len(),
        report.source_final_event_hash,
    );
    for finding in &report.findings {
        writeln!(
            output,
            "finding: {} [{:?}] states={:?} tokens={:?}",
            finding.code, finding.confidence, finding.state_ids, finding.token_ids,
        )
        .into_diagnostic()?;
        for fact in &finding.facts {
            writeln!(output, "  fact: {fact}").into_diagnostic()?;
        }
        for correlation in &finding.native_correlations {
            writeln!(
                output,
                "  native: {:?} state={} frame={:?} {}",
                correlation.level, correlation.state_id, correlation.frame, correlation.limitation,
            )
            .into_diagnostic()?;
        }
    }
    return Ok(output);
}

fn diff_point(
    state: Option<u64>,
    event_seq: Option<u64>,
    monotonic_ns: Option<u64>,
    marker: Option<String>,
) -> Result<StatePoint> {
    return match (state, event_seq, monotonic_ns, marker) {
        (Some(state_id), None, None, None) => Ok(StatePoint::StateId { state_id }),
        (None, Some(event_seq), None, None) => Ok(StatePoint::EventSeq { event_seq }),
        (None, None, Some(monotonic_ns), None) => Ok(StatePoint::MonotonicNs { monotonic_ns }),
        (None, None, None, Some(label)) => Ok(StatePoint::Marker { label }),
        _ => Err(miette::miette!(
            "exactly one diff endpoint selector is required"
        )),
    };
}

fn diff_text(difference: &SemanticDiff) -> Result<String> {
    let changed_cells = difference.rows.iter().fold(0_usize, |count, row| {
        return count.saturating_add(row.cells.len());
    });
    let mut output = format!(
        "states {} -> {}  changed={}  rows={}  cells={}\n",
        difference.from.state_id,
        difference.to.state_id,
        difference.changed,
        difference.rows.len(),
        changed_cells,
    );
    for row in &difference.rows {
        for cell in &row.cells {
            let before = cell
                .before
                .as_ref()
                .map_or("∅", |value| return value.grapheme.as_str());
            let after = cell
                .after
                .as_ref()
                .map_or("∅", |value| return value.grapheme.as_str());
            writeln!(
                output,
                "  ({},{}) {:?} -> {:?}",
                cell.column, row.row, before, after
            )
            .into_diagnostic()?;
        }
    }
    return Ok(output);
}

fn timeline_text(entry: &TimelineEntry) -> String {
    let seconds = entry.monotonic_ns / 1_000_000_000;
    let fractional = entry.monotonic_ns % 1_000_000_000;
    let event = entry
        .event_seq
        .map_or_else(|| return "-".to_owned(), |seq| return seq.to_string());
    let detail = match &entry.item {
        TimelineItem::SemanticState {
            state_id,
            event_byte_offset,
            presented,
            changed_rows,
            changed_cells,
            cursor_changed,
            modes_changed,
            metadata_changed,
            ..
        } => format!(
            "state {state_id} byte {} presented={} rows={} cells={} cursor={} modes={} metadata={}",
            event_byte_offset
                .map_or_else(|| return "-".to_owned(), |value| return value.to_string()),
            if *presented { "yes" } else { "no" },
            changed_rows.len(),
            changed_cells,
            cursor_changed,
            modes_changed,
            metadata_changed,
        ),
        TimelineItem::CanonicalEvent { event } => canonical_event_text(event),
        TimelineItem::VtToken { token } => {
            let location = token.source.first().map_or_else(
                || return "-".to_owned(),
                |segment| {
                    return format!("{}..{}", segment.start_byte_offset, segment.end_byte_offset);
                },
            );
            format!(
                "token {} {:?} bytes={} range={} {}",
                token.token_id,
                token.kind,
                token.raw.as_bytes().len(),
                location,
                token.description,
            )
        }
    };
    return format!("{seconds}.{fractional:09}s  event {event}  {detail}");
}

fn canonical_event_text(event: &Event) -> String {
    return match event {
        Event::InputIntent {
            source,
            input_kind,
            data,
            intent,
            ..
        } => format!(
            "input {source:?}/{input_kind:?} bytes={}{}",
            data.as_bytes().len(),
            intent
                .as_ref()
                .map_or_else(String::new, |value| return format!(" intent={value:?}"))
        ),
        Event::ResizeIntent { cols, rows, .. } => {
            format!("resize {cols}x{rows}")
        }
        Event::SignalIntent { name, .. } => format!("signal {name}"),
        Event::Marker { label, .. } => format!("marker {label:?}"),
        Event::QueryResponse {
            query, responder, ..
        } => format!(
            "query bytes={} responder={responder}",
            query.as_bytes().len()
        ),
        Event::Lifecycle {
            lifecycle_kind,
            pid,
        } => format!("lifecycle {lifecycle_kind:?} pid={pid:?}"),
        Event::Exit {
            code, signal_name, ..
        } => format!("exit code={code:?} signal={signal_name:?}"),
        Event::HostError { code, message } => {
            format!("host_error {code}: {message}")
        }
        _ => "canonical operation result".to_owned(),
    };
}

struct OneOffCommandOutcome {
    session: PathBuf,
    observation: Option<Observation>,
    detached: bool,
    diagnostic_logs: Vec<PathBuf>,
}

fn record_ansi_import(
    bytes: &[u8],
    source: &Path,
    destination: &Path,
    cols: u16,
    rows: u16,
) -> Result<()> {
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        }
        .into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return miette::miette!("ANSI import destination has no parent"))?;
    fs_err::create_dir_all(parent).into_diagnostic()?;
    let input = parent.join(format!(".ansi-import-{}.tmp", uuid::Uuid::now_v7()));
    write_secure(&input, bytes)?;
    let hash = blake3::hash(bytes).to_hex().to_string();
    let process = ProcessCommand::new(env::current_exe().into_diagnostic()?)
        .args(["record", "--no-input", "--out"])
        .arg(destination)
        .args(["--cols", &cols.to_string(), "--rows", &rows.to_string()])
        .args(["--import-source-kind", "ansi_stream"])
        .args(["--import-source-byte-count", &bytes.len().to_string()])
        .args(["--import-source-blake3", &hash])
        .arg("--import-source")
        .arg(source.as_os_str())
        .args([
            "--",
            "/bin/sh",
            "-c",
            "stty raw -echo < /dev/tty; exec /bin/cat -- \"$1\"",
            "termtrace-ansi-import",
        ])
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let cleanup = fs_err::remove_file(&input);
    let process = process.into_diagnostic()?;
    cleanup.into_diagnostic()?;
    if !process.status.success() {
        return Err(miette::miette!(
            "ANSI import recorder failed with {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr)
        ));
    }
    let log = EventLog::read(destination.join("events.jsonl"))?;
    validate_completed_log(destination, &log)?;
    let imported = log
        .events
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            };
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    if imported != bytes {
        return Err(miette::miette!(
            "ANSI import canonical output does not match the exact source bytes"
        ));
    }
    let manifest = read_manifest(destination)?;
    if manifest
        .import_provenance
        .as_ref()
        .is_none_or(|provenance| {
            return provenance.source_kind != "ansi_stream"
                || provenance.byte_count != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                || provenance.blake3 != hash;
        })
    {
        return Err(miette::miette!(
            "ANSI import session did not bind its source provenance"
        ));
    }
    return Ok(());
}

fn run_one_off_command(
    command: &[OsString],
    cols: u16,
    rows: u16,
    wait_predicate: Option<ScreenPredicate>,
    stable_for: Duration,
    timeout: Duration,
    detach: bool,
    requested_out: Option<&Path>,
) -> Result<OneOffCommandOutcome> {
    if cols == 0 || rows == 0 {
        return Err(miette::miette!(
            "one-off geometry must be greater than zero"
        ));
    }
    let paths = if let Some(session) = requested_out {
        if session.exists() {
            return Err(Error::ArtifactExists {
                path: session.to_path_buf(),
            }
            .into());
        }
        let parent = session
            .parent()
            .ok_or_else(|| return miette::miette!("one-off --out path has no parent"))?;
        fs_err::create_dir_all(parent).into_diagnostic()?;
        let name = session
            .file_name()
            .ok_or_else(|| return miette::miette!("one-off --out path has no basename"))?
            .to_string_lossy();
        let identity = uuid::Uuid::now_v7();
        DetachedSessionPaths {
            session: session.to_path_buf(),
            stdout_log: parent.join(format!(".{name}.{identity}.stdout.log")),
            stderr_log: parent.join(format!(".{name}.{identity}.stderr.log")),
        }
    } else {
        SessionRegistry::open_default()?.allocate_detached_paths()?
    };
    let stdout = open_detached_log(&paths.stdout_log)?;
    let stderr = open_detached_log(&paths.stderr_log)?;
    let executable = env::current_exe().into_diagnostic()?;
    let mut child = ProcessCommand::new(executable)
        .arg("record")
        .arg("--no-input")
        .arg("--out")
        .arg(&paths.session)
        .arg("--cols")
        .arg(cols.to_string())
        .arg("--rows")
        .arg(rows.to_string())
        .arg("--")
        .args(command)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .into_diagnostic()?;
    let socket = paths.session.join("control.sock");
    let started = Instant::now();
    let deadline = started + timeout;
    while !socket.exists() {
        if let Some(status) = child.try_wait().into_diagnostic()? {
            if paths.session.join("completion.json").is_file() {
                let mut observation =
                    observe_one_off_session(&paths.session, StatePoint::LatestPresented, false)?;
                if let Some(predicate) = &wait_predicate {
                    let matched = match predicate {
                        ScreenPredicate::Contains { text } => {
                            observation.frame.viewport_text.contains(text)
                        }
                        ScreenPredicate::MatchesRegex { pattern } => regex::Regex::new(pattern)
                            .into_diagnostic()?
                            .is_match(&observation.frame.viewport_text),
                        _ => false,
                    };
                    if !matched {
                        return Err(miette::miette!(
                            "one-off command completed before its screen predicate matched; completed evidence: {}",
                            paths.session.display(),
                        ));
                    }
                }
                observation.source_kind = ObservationSourceKind::OneOffCommand;
                observation.source_completed = true;
                return Ok(OneOffCommandOutcome {
                    session: paths.session,
                    observation: Some(observation),
                    detached: false,
                    diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
                });
            }
            return Err(miette::miette!(
                "one-off recorder exited with {status} before control readiness; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "one-off recorder did not become ready before the deadline; recoverable session candidate: {}",
                paths.session.display(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    if detach {
        return Ok(OneOffCommandOutcome {
            session: paths.session,
            observation: None,
            detached: true,
            diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
        });
    }
    let mut observation = if let Some(predicate) = wait_predicate {
        let waited = match wait_for_screen(
            &paths.session,
            predicate,
            stable_for,
            timeout.saturating_sub(started.elapsed()),
        ) {
            Ok(waited) => waited,
            Err(error) => {
                let stop_result = request(&paths.session, ControlCommand::Stop);
                if stop_result.is_ok() {
                    let _ = child.wait();
                }
                let stop_description = match &stop_result {
                    Ok(_) => "accepted".to_owned(),
                    Err(stop_error) => stop_error.to_string(),
                };
                return Err(miette::miette!(
                    "one-off wait failed: {error}; evidence preserved at {}; stop request: {}",
                    paths.session.display(),
                    stop_description,
                ));
            }
        };
        let observation = match observe_session_with_options(
            &paths.session,
            StatePoint::LatestPresented,
            false,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let stop_result = request(&paths.session, ControlCommand::Stop);
                if stop_result.is_ok() {
                    let _ = child.wait();
                }
                let stop_description = match &stop_result {
                    Ok(_) => "accepted".to_owned(),
                    Err(stop_error) => stop_error.to_string(),
                };
                return Err(miette::miette!(
                    "one-off observation failed after its wait proof: {error}; evidence preserved at {}; stop request: {}",
                    paths.session.display(),
                    stop_description,
                ));
            }
        };
        if observation.state_hash != waited.state_hash {
            let stop_result = request(&paths.session, ControlCommand::Stop);
            if stop_result.is_ok() {
                let _ = child.wait();
            }
            let stop_description = match &stop_result {
                Ok(_) => "accepted".to_owned(),
                Err(stop_error) => stop_error.to_string(),
            };
            return Err(miette::miette!(
                "one-off presented state changed after its wait proof; evidence preserved at {}; stop request: {}",
                paths.session.display(),
                stop_description,
            ));
        }
        request(&paths.session, ControlCommand::Stop)?;
        Some(observation)
    } else {
        None
    };
    let status = child.wait().into_diagnostic()?;
    if !paths.session.join("completion.json").is_file() {
        return Err(miette::miette!(
            "one-off recorder exited with {status} before durable completion; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
            paths.session.display(),
            paths.stdout_log.display(),
            paths.stderr_log.display(),
        ));
    }
    if let Some(observation) = &mut observation {
        observation.source_kind = ObservationSourceKind::OneOffCommand;
        observation.source_completed = true;
    }
    return Ok(OneOffCommandOutcome {
        session: paths.session,
        observation,
        detached: false,
        diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
    });
}

fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let timeout = Duration::from_millis(arguments.timeout_ms);
    if let Some(output) = arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }
        return write_json_line(&wait_for_output(session, output.as_bytes(), timeout)?);
    }
    let predicate = match (
        arguments.screen,
        arguments.screen_absent,
        arguments.screen_regex,
        arguments.row_contains,
        arguments.region_contains,
        arguments.cursor_at,
        arguments.cursor_visible,
        arguments.cursor_shape,
        arguments.mode,
        arguments.mode_disabled,
        arguments.state_changed,
        arguments.state_unchanged,
    ) {
        (Some(text), None, None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Contains { text }
        }
        (None, Some(text), None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Absent { text }
        }
        (None, None, Some(pattern), None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::MatchesRegex { pattern }
        }
        (None, None, None, Some(value), None, None, None, None, None, None, None, None) => {
            let (row, text) = parse_row_predicate(&value)?;
            ScreenPredicate::RowContains { row, text }
        }
        (None, None, None, None, Some(value), None, None, None, None, None, None, None) => {
            let (region, text) = parse_region_predicate(&value)?;
            ScreenPredicate::RegionContains { region, text }
        }
        (None, None, None, None, None, Some(value), None, None, None, None, None, None) => {
            let (column, row) = parse_cursor_position(&value)?;
            ScreenPredicate::Cursor {
                column: Some(column),
                row: Some(row),
                visible: None,
                shape: None,
            }
        }
        (None, None, None, None, None, None, Some(visible), None, None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: Some(visible),
                shape: None,
            }
        }
        (None, None, None, None, None, None, None, Some(shape), None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: None,
                shape: Some(shape.into()),
            }
        }
        (None, None, None, None, None, None, None, None, Some(mode), None, None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: true,
            }
        }
        (None, None, None, None, None, None, None, None, None, Some(mode), None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: false,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, Some(hash), None) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Changed,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, None, Some(hash)) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Unchanged,
            }
        }
        _ => return Err(miette::miette!("exactly one wait predicate is required")),
    };
    return write_json_line(&wait_for_screen(
        session,
        predicate,
        arguments.stable_for,
        timeout,
    )?);
}

fn parse_row_predicate(value: &str) -> Result<(u16, String)> {
    let (row, text) = value
        .split_once(':')
        .ok_or_else(|| return miette::miette!("row predicate must use ROW:TEXT"))?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return miette::miette!("row predicate ROW must be a zero-based integer"))?;
    return Ok((row, text.to_owned()));
}

fn parse_region_predicate(value: &str) -> Result<(ScreenRegion, String)> {
    let (bounds, text) = value.split_once(':').ok_or_else(|| {
        return miette::miette!("region predicate must use LEFT,TOP,RIGHT,BOTTOM:TEXT");
    })?;
    let values = bounds
        .split(',')
        .map(|item| {
            return item.parse::<u16>().map_err(|_| {
                return miette::miette!("region bounds must be zero-based integers");
            });
        })
        .collect::<Result<Vec<_>>>()?;
    let [left, top, right, bottom] = values.as_slice() else {
        return Err(miette::miette!(
            "region predicate requires exactly four bounds"
        ));
    };
    return Ok((
        ScreenRegion {
            left: *left,
            top: *top,
            right: *right,
            bottom: *bottom,
        },
        text.to_owned(),
    ));
}

fn parse_cursor_position(value: &str) -> Result<(u16, u16)> {
    let (column, row) = value
        .split_once(',')
        .ok_or_else(|| return miette::miette!("cursor position must use COLUMN,ROW"))?;
    let column = column
        .parse::<u16>()
        .map_err(|_| return miette::miette!("cursor COLUMN must be a zero-based integer"))?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return miette::miette!("cursor ROW must be a zero-based integer"))?;
    return Ok((column, row));
}

fn run_recover(arguments: RecoverArgs) -> Result<ExitCode> {
    let report = EventLog::recover_prefix(arguments.event_log, arguments.out)?;
    return write_stdout(format!(
        "{}\n",
        serde_json::to_string_pretty(&report).into_diagnostic()?
    ));
}

fn run_schemas(arguments: SchemaArgs) -> Result<ExitCode> {
    let mut output = String::new();
    for path in write_schemas(arguments.out)? {
        writeln!(output, "{}", path.display()).into_diagnostic()?;
    }
    return write_stdout(output);
}

fn run_control(session: PathBuf, command: ControlCommand) -> Result<ExitCode> {
    return run_control_guarded(session, command, None);
}

fn run_control_guarded(
    session: PathBuf,
    command: ControlCommand,
    expected_presented_state_hash: Option<String>,
) -> Result<ExitCode> {
    let session = resolve_session_ref(session)?;
    let mut control_request = ControlRequest::new(command);
    control_request.expected_presented_state_hash = expected_presented_state_hash;
    let data = send_request(session, &control_request)?;
    return write_json_line(&data);
}

fn run_status(arguments: StatusArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let completion_path = session.join("completion.json");
    if arguments.wait_complete || completion_path.is_file() {
        let completion =
            wait_for_completion(&session, Duration::from_millis(arguments.timeout_ms))?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    let data = request(session, ControlCommand::Status)?;
    return write_json_line(&data);
}

fn run_stop(arguments: StopArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if session.join("completion.json").is_file() {
        let completion = wait_for_completion(&session, Duration::ZERO)?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    let mut control_request = ControlRequest::new(ControlCommand::Stop);
    control_request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let acknowledged = send_request(&session, &control_request)?;
    if !arguments.wait {
        return write_json_line(&acknowledged);
    }
    let completion = wait_for_completion(&session, Duration::from_millis(arguments.timeout_ms))?;
    return write_json_line(&serde_json::json!({
        "kind": "completed",
        "completion": completion,
    }));
}

fn resolve_session_ref(target: PathBuf) -> Result<PathBuf> {
    let bytes = target.as_os_str().as_bytes();
    if let Some(path) = bytes.strip_prefix(b"path:") {
        if path.is_empty() {
            return Err(miette::miette!("path: session reference is empty"));
        }
        return Ok(PathBuf::from(OsString::from_vec(path.to_vec())));
    }
    if let Some(name) = bytes.strip_prefix(b"name:") {
        let name = str::from_utf8(name)
            .map_err(|_| miette::miette!("name: session reference must be valid UTF-8"))?;
        return Ok(SessionRegistry::open_default()?
            .resolve(name)?
            .session_path());
    }
    if bytes.contains(&b'/') || bytes.starts_with(b".") {
        return Ok(target);
    }
    let name = target
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 session paths require the path: prefix"))?;
    return Ok(SessionRegistry::open_default()?
        .resolve(name)?
        .session_path());
}

fn resolve_cli_range(session: &Path, range: &TimeRangeArgs) -> Result<(Option<u64>, Option<u64>)> {
    if range.from_marker.is_none() && range.to_marker.is_none() {
        return Ok((range.start_monotonic_ns, range.end_monotonic_ns));
    }
    let event_path = session.join("events.jsonl");
    let live_event_count = if !session.join("completion.json").is_file() {
        match request(session, ControlCommand::Status) {
            Ok(ControlData::Status { boundaries, .. }) => Some(boundaries.durable.next_event_seq),
            Ok(_) => {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            }
            Err(error) => {
                tracing::debug!(%error, "marker selection has no responsive control boundary");
                None
            }
        }
    } else {
        None
    };
    let log = match live_event_count {
        Some(event_count) => EventLog::read_prefix(event_path, event_count)?,
        None => EventLog::read(event_path)?,
    };
    let resolved = resolve_source_selection(
        &log,
        range.start_monotonic_ns,
        range.end_monotonic_ns,
        range.from_marker.as_deref(),
        range.to_marker.as_deref(),
    )?;
    return Ok(resolved.map_or((None, None), |value| {
        return (Some(value.start_monotonic_ns), Some(value.end_monotonic_ns));
    }));
}

fn selection(presented_only: bool) -> StateSelection {
    if presented_only {
        return StateSelection::Presented;
    }
    return StateSelection::All;
}

fn row_text(row: &RowState) -> String {
    let mut text = String::new();
    for cell in &row.cells {
        if cell.grapheme.is_empty() {
            text.push(' ');
        } else {
            text.push_str(&cell.grapheme);
        }
    }
    return text.trim_end().to_owned();
}

fn write_json_line(value: &impl serde::Serialize) -> Result<ExitCode> {
    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');
    return write_stdout(encoded);
}

fn write_stdout(output: impl AsRef<[u8]>) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    match stdout
        .write_all(output.as_ref())
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}

fn run_send(arguments: SendArgs) -> Result<ExitCode> {
    if arguments.stdin {
        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).into_diagnostic()?;
        return run_control_guarded(
            arguments.session,
            ControlCommand::Input {
                data: ByteString::new(data),
                input_kind: InputKind::Raw,
                intent: arguments.intent,
            },
            arguments.expected_presented_state_hash,
        );
    }
    let session = resolve_session_ref(arguments.session)?;
    let mut commands = Vec::new();
    for atom in &arguments.atoms {
        if arguments.pace.is_zero() || !atom.starts_with("text:") {
            commands.push(control_command_for_atom(atom, arguments.intent.clone())?);
            continue;
        }
        let text = atom.strip_prefix("text:").unwrap_or_default();
        for scalar in text.chars() {
            commands.push(ControlCommand::Input {
                data: ByteString::new(scalar.to_string().into_bytes()),
                input_kind: InputKind::Text,
                intent: arguments.intent.clone(),
            });
        }
    }
    if commands.is_empty() || commands.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let sequence_id = uuid::Uuid::now_v7();
    let mut outcomes = Vec::with_capacity(commands.len());
    for (index, command) in commands.into_iter().enumerate() {
        if index != 0 && !arguments.pace.is_zero() {
            thread::sleep(arguments.pace);
        }
        let mut request = ControlRequest::new(command);
        request.expected_presented_state_hash = arguments.expected_presented_state_hash.clone();
        let data = send_request(&session, &request)?;
        outcomes.push(serde_json::json!({
            "atom_id": request.request_id,
            "status": "applied",
            "result": data,
        }));
    }
    return write_json_line(&serde_json::json!({
        "kind": "input_sequence",
        "sequence_id": sequence_id,
        "pace_ns": arguments.pace.as_nanos(),
        "atoms": outcomes,
    }));
}

fn control_command_for_atom(atom: &str, intent: Option<String>) -> Result<ControlCommand> {
    if let Some((key, modifiers)) = parse_named_key(atom)? {
        return Ok(ControlCommand::Key { key, modifiers });
    }
    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(ControlCommand::Paste {
            data: ByteString::new(paste.as_bytes().to_vec()),
        });
    }
    let (data, input_kind) = if let Some(text) = atom.strip_prefix("text:") {
        (text.as_bytes().to_vec(), InputKind::Text)
    } else if let Some(raw) = atom.strip_prefix("raw:") {
        (raw.as_bytes().to_vec(), InputKind::Raw)
    } else if let Some(path) = atom.strip_prefix("raw-file:") {
        (fs_err::read(path).into_diagnostic()?, InputKind::Raw)
    } else {
        return Err(miette::miette!(
            "unknown input atom; use a named key, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH"
        ));
    };
    return Ok(ControlCommand::Input {
        data: ByteString::new(data),
        input_kind,
        intent,
    });
}

fn parse_event_type(value: &str) -> StdResult<String, String> {
    const EVENT_TYPES: &[&str] = &[
        "output",
        "input_intent",
        "input_result",
        "resize_intent",
        "resize_result",
        "signal_intent",
        "signal_result",
        "marker",
        "query_response",
        "lifecycle",
        "exit",
        "host_error",
    ];
    if EVENT_TYPES.contains(&value) {
        return Ok(value.to_owned());
    }
    return Err(format!(
        "unknown event type `{value}`; expected one of {}",
        EVENT_TYPES.join(",")
    ));
}

fn parse_capability_id(value: &str) -> StdResult<String, String> {
    const CAPABILITIES: &[&str] = &[
        "core",
        "semantic",
        "native-screenshot",
        "native-video",
        "presentation",
        "receipt-signing",
    ];
    if CAPABILITIES.contains(&value) {
        return Ok(value.to_owned());
    }
    return Err(format!(
        "unknown capability `{value}`; expected one of {}",
        CAPABILITIES.join(",")
    ));
}

fn parse_diagnostic_check(value: &str) -> StdResult<DiagnosticCheck, String> {
    return match value {
        "full-redraw" => Ok(DiagnosticCheck::FullRedraw),
        "transient" | "flicker" => Ok(DiagnosticCheck::Transient),
        "dirty" | "stale-cells" => Ok(DiagnosticCheck::Dirty),
        "cursor" => Ok(DiagnosticCheck::Cursor),
        "modes" => Ok(DiagnosticCheck::Modes),
        "palette" => Ok(DiagnosticCheck::Palette),
        "query-loop" => Ok(DiagnosticCheck::QueryLoop),
        _ => Err(format!(
            "unknown diagnostic check `{value}`; expected full-redraw, transient, dirty, cursor, modes, palette, or query-loop"
        )),
    };
}

fn parse_pace_duration(value: &str) -> StdResult<Duration, String> {
    let nanoseconds = parse_time_offset_ns(value)?;
    if nanoseconds > 60_000_000_000 {
        return Err("pace must not exceed 60s".to_owned());
    }
    return Ok(Duration::from_nanos(nanoseconds));
}

fn parse_scenario_jobs(value: &str) -> StdResult<usize, String> {
    let jobs = parse_positive_usize(value)?;
    if jobs > 64 {
        return Err("scenario jobs must be between 1 and 64".to_owned());
    }
    return Ok(jobs);
}

fn parse_positive_usize(value: &str) -> StdResult<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_error| return "value must be a positive integer".to_owned())?;
    if parsed == 0 {
        return Err("value must be greater than zero".to_owned());
    }
    return Ok(parsed);
}

fn parse_cell(value: &str) -> StdResult<(u16, u16), String> {
    let (column, row) = value
        .split_once(',')
        .ok_or_else(|| return "cell must use zero-based COLUMN,ROW syntax".to_owned())?;
    let column = column
        .parse::<u16>()
        .map_err(|_| return "cell column must be an unsigned 16-bit integer".to_owned())?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return "cell row must be an unsigned 16-bit integer".to_owned())?;
    return Ok((column, row));
}

fn parse_named_key(atom: &str) -> Result<Option<(NamedKey, KeyModifiers)>> {
    let mut modifiers = KeyModifiers::default();
    let key = match atom {
        "enter" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "shift-tab" => {
            modifiers.shift = true;
            NamedKey::Tab
        }
        "backspace" => NamedKey::Backspace,
        "delete" => NamedKey::Delete,
        "insert" => NamedKey::Insert,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "page-up" => NamedKey::PageUp,
        "page-down" => NamedKey::PageDown,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        value if value.len() >= 2 && value.starts_with('f') => {
            let suffix = value
                .strip_prefix('f')
                .ok_or_else(|| miette::miette!("function key must be f1 through f24"))?;
            let number = suffix
                .parse::<u8>()
                .map_err(|_| miette::miette!("function key must be f1 through f24"))?;
            if !(1..=24).contains(&number) {
                return Err(miette::miette!("function key must be f1 through f24"));
            }
            NamedKey::Function(number)
        }
        value if value.len() == 6 && value.starts_with("ctrl-") => {
            let letter = value
                .chars()
                .nth(5)
                .ok_or_else(|| miette::miette!("control key must be ctrl-a through ctrl-z"))?;
            if !letter.is_ascii_alphabetic() {
                return Err(miette::miette!("control key must be ctrl-a through ctrl-z"));
            }
            modifiers.control = true;
            NamedKey::Letter(letter)
        }
        _ => return Ok(None),
    };
    return Ok(Some((key, modifiers)));
}

fn run_resize(arguments: ResizeArgs) -> Result<ExitCode> {
    let pixel_width = arguments
        .pixel_width
        .or_else(|| return arguments.cols.checked_mul(9))
        .ok_or_else(|| miette::miette!("pixel width overflow; pass --pixel-width explicitly"))?;
    let pixel_height = arguments
        .pixel_height
        .or_else(|| return arguments.rows.checked_mul(18))
        .ok_or_else(|| miette::miette!("pixel height overflow; pass --pixel-height explicitly"))?;
    return run_control_guarded(
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
        arguments.expected_presented_state_hash,
    );
}

fn run_pointer(arguments: PointerArgs) -> Result<ExitCode> {
    let (column, row, operation, strict, expected_presented_state_hash, postcondition) =
        match arguments.operation {
            PointerCommand::Click(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::Click {
                    button: click.button.into(),
                },
                click.strict,
                click.expected_presented_state_hash,
                click.postcondition,
            ),
            PointerCommand::MultiClick(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::MultiClick {
                    button: click.button.into(),
                    count: click.count,
                },
                click.strict,
                click.expected_presented_state_hash,
                click.postcondition,
            ),
            PointerCommand::Drag(drag) => (
                drag.from.0,
                drag.from.1,
                PointerOperation::Drag {
                    button: drag.button.into(),
                    to_column: drag.to.0,
                    to_row: drag.to.1,
                    steps: drag.steps,
                },
                drag.strict,
                drag.expected_presented_state_hash,
                drag.postcondition,
            ),
            PointerCommand::Scroll(scroll) => (
                scroll.cell.0,
                scroll.cell.1,
                PointerOperation::Scroll {
                    lines: scroll.lines,
                },
                scroll.strict,
                scroll.expected_presented_state_hash,
                scroll.postcondition,
            ),
        };
    let session = resolve_session_ref(arguments.session)?;
    let mut request = ControlRequest::new(ControlCommand::Pointer {
        column,
        row,
        operation,
        strict,
    });
    request.expected_presented_state_hash = expected_presented_state_hash;
    let input = send_request(&session, &request)?;
    let predicate = match (postcondition.wait_screen, postcondition.wait_screen_absent) {
        (Some(text), None) => Some(ScreenPredicate::Contains { text }),
        (None, Some(text)) => Some(ScreenPredicate::Absent { text }),
        (None, None) => None,
        (Some(_), Some(_)) => {
            return Err(miette::miette!(
                "pointer postcondition requires exactly one screen predicate"
            ));
        }
    };
    let Some(predicate) = predicate else {
        return write_json_line(&input);
    };
    let matched = wait_for_screen(
        &session,
        predicate,
        postcondition.stable_for,
        Duration::from_millis(postcondition.timeout_ms),
    )?;
    return write_json_line(&serde_json::json!({
        "kind": "pointer_with_postcondition",
        "input": input,
        "postcondition": matched,
    }));
}

fn run_record(arguments: RecordArgs) -> Result<ExitCode> {
    let RecordArgs {
        out,
        no_input,
        session_alias,
        restart_source,
        restart_overrides,
        import_source_kind,
        import_source_byte_count,
        import_source_blake3,
        import_source,
        recording: arguments,
    } = arguments;
    let import_provenance = match (
        import_source_kind,
        import_source_byte_count,
        import_source_blake3,
        import_source,
    ) {
        (Some(source_kind), Some(byte_count), Some(blake3), Some(source)) => {
            if source_kind != "ansi_stream" || blake3.len() != 64 {
                return Err(miette::miette!("invalid internal ANSI import provenance"));
            }
            Some(ImportProvenance {
                source_kind,
                byte_count,
                blake3,
                source: ByteString::from(source.as_os_str().as_bytes()),
            })
        }
        (None, None, None, None) => None,
        _ => {
            return Err(miette::miette!(
                "incomplete internal ANSI import provenance"
            ));
        }
    };
    if let Some(predecessor) = restart_source {
        if import_provenance.is_some() {
            return Err(miette::miette!(
                "restart cannot carry ANSI import provenance"
            ));
        }
        let overrides = restart_overrides
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .into_diagnostic()?
            .unwrap_or_else(empty_restart_overrides);
        let options = restarted_record_options(out, session_alias, &predecessor, &overrides)?;
        return report_record_outcome(record(&options)?);
    }
    if matches!(arguments.durability, DurabilityModeArg::Bounded)
        && (arguments.durability_max_interval_ms == 0
            || arguments.durability_max_uncommitted_bytes == 0)
    {
        return Err(miette::miette!(
            "bounded durability thresholds must be greater than zero"
        ));
    }
    let durability_policy = arguments.durability.policy(
        arguments.durability_max_interval_ms,
        arguments.durability_max_uncommitted_bytes,
    );
    if !matches!(arguments.provenance, ProvenancePolicyArg::Custom)
        && !arguments.provenance_environment.is_empty()
    {
        return Err(miette::miette!(
            "--provenance-env requires --provenance custom"
        ));
    }
    let provenance_policy = ProvenancePolicy {
        kind: arguments.provenance.into(),
        environment: arguments.provenance_environment.iter().cloned().collect(),
    };
    let overrides = GhosttyVisualOverrides {
        font_family: arguments.font_family.clone(),
        font_size: arguments.font_size.clone(),
        padding_x: arguments.padding_x,
        padding_y: arguments.padding_y,
        foreground: arguments.foreground.clone(),
        background: arguments.background.clone(),
    };
    let needs_profile = arguments.ghostty_config.is_some()
        || !overrides.is_empty()
        || matches!(arguments.geometry, GeometryMode::Fit);
    let ghostty_profile = if needs_profile {
        Some(resolve_ghostty_profile(
            arguments.ghostty_config.as_deref(),
            overrides,
        )?)
    } else {
        None
    };
    let (geometry, geometry_provenance) =
        resolve_record_geometry(&arguments, ghostty_profile.as_ref())?;
    let cell_width_px = geometry
        .pixel_width
        .checked_div(geometry.cols)
        .ok_or_else(|| miette::miette!("resolved terminal has no columns"))?;
    let cell_height_px = geometry
        .pixel_height
        .checked_div(geometry.rows)
        .ok_or_else(|| miette::miette!("resolved terminal has no rows"))?;
    let colors = ghostty_profile.as_ref().map_or_else(
        || return resolved_terminal_colors(),
        |profile| return terminal_colors_for_profile(profile),
    )?;
    let outcome = record(&RecordOptions {
        output_directory: out,
        creation_alias: session_alias,
        lineage: None,
        import_provenance,
        command: arguments.command,
        working_directory: arguments.cwd,
        cols: geometry.cols,
        rows: geometry.rows,
        cell_width_px,
        cell_height_px,
        geometry_provenance,
        ghostty_profile,
        term: arguments.term,
        colors,
        forward_stdin: !no_input,
        durability_policy,
        provenance_policy,
    })?;
    return report_record_outcome(outcome);
}

fn report_record_outcome(outcome: RecordOutcome) -> Result<ExitCode> {
    writeln!(io::stderr(), "recording: {}", outcome.event_log.display()).into_diagnostic()?;
    return match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => Ok(ExitCode::FAILURE),
    };
}

fn run_start(arguments: StartArgs) -> Result<ExitCode> {
    let StartArgs {
        name,
        startup_timeout_ms,
        recording: _recording,
    } = arguments;
    if startup_timeout_ms == 0 {
        return Err(miette::miette!(
            "--startup-timeout-ms must be greater than zero"
        ));
    }
    let registry = SessionRegistry::open_default()?;
    registry.ensure_available(&name)?;
    let paths = registry.allocate_detached_paths()?;
    let stdout = open_detached_log(&paths.stdout_log)?;
    let stderr = open_detached_log(&paths.stderr_log)?;
    let executable = env::current_exe().into_diagnostic()?;
    let record_arguments = detached_record_arguments(&name, &paths.session)?;
    let mut recorder = ProcessCommand::new("setsid")
        .arg(executable)
        .args(record_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .into_diagnostic()?;
    let deadline = Instant::now() + Duration::from_millis(startup_timeout_ms);
    loop {
        if paths.session.join("session.json").is_file()
            && registry.recorder_identity(&paths.session).is_some()
        {
            let entry = registry.register(&name, &paths.session).map_err(|error| {
                return miette::miette!(
                    "recorder became ready but alias publication failed; recoverable orphan: {}; stdout log: {}; stderr log: {}; {error}",
                    paths.session.display(),
                    paths.stdout_log.display(),
                    paths.stderr_log.display(),
                );
            })?;
            return write_json_line(&serde_json::json!({
                "alias": entry.alias,
                "canonical_session": entry.canonical_session,
                "session_id": entry.session_id,
                "recorder_pid": entry.recorder_pid,
                "recorder_process_start_ticks": entry.recorder_process_start_ticks,
                "stdout_log": paths.stdout_log,
                "stderr_log": paths.stderr_log,
                "ready": true,
            }));
        }
        if let Some(status) = recorder.try_wait().into_diagnostic()? {
            return Err(miette::miette!(
                "detached recorder exited before readiness with {status}; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "detached recorder did not become ready within {startup_timeout_ms} ms; it was left running as recoverable orphan: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_restart(arguments: RestartArgs) -> Result<ExitCode> {
    if arguments.timeout_ms == 0 {
        return Err(miette::miette!("--timeout-ms must be greater than zero"));
    }
    let registry = SessionRegistry::open_default()?;
    let overrides = restart_overrides_from_args(&arguments)?;
    let encoded_overrides = serde_json::to_string(&overrides).into_diagnostic()?;
    let predecessor_entry = registry.resolve(&arguments.name)?;
    let predecessor = predecessor_entry.session_path();
    let timeout = Duration::from_millis(arguments.timeout_ms);
    if !predecessor.join("completion.json").is_file() {
        request(&predecessor, ControlCommand::Stop)?;
    }
    let completion = wait_for_completion(&predecessor, timeout)?;
    let predecessor_manifest_bytes =
        fs_err::read(predecessor.join("session.json")).into_diagnostic()?;
    let expected_lineage = SessionLineage {
        predecessor_session_id: predecessor_entry.session_id,
        predecessor_manifest_hash: manifest_hash(&predecessor_manifest_bytes),
        predecessor_final_event_hash: completion.final_event_hash,
        reason: "restart".to_owned(),
    };

    let paths = registry.allocate_detached_paths()?;
    let stdout = open_detached_log(&paths.stdout_log)?;
    let stderr = open_detached_log(&paths.stderr_log)?;
    let executable = env::current_exe().into_diagnostic()?;
    let mut recorder = ProcessCommand::new("setsid")
        .arg(executable)
        .args(restart_record_arguments(
            &arguments.name,
            &paths.session,
            &predecessor,
            &encoded_overrides,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .into_diagnostic()?;
    let deadline = Instant::now() + timeout;
    loop {
        if paths.session.join("session.json").is_file()
            && registry.recorder_identity(&paths.session).is_some()
        {
            let successor_manifest = read_manifest(&paths.session)?;
            if successor_manifest.lineage.as_ref() != Some(&expected_lineage) {
                return Err(miette::miette!(
                    "successor lineage does not match the completed predecessor; alias remains unchanged; recoverable orphan: {}",
                    paths.session.display()
                ));
            }
            let successor = registry
                .retarget(
                    &arguments.name,
                    predecessor_entry.session_id,
                    &paths.session,
                )
                .map_err(|error| {
                    return miette::miette!(
                        "successor became ready but alias retargeting failed; alias remains unchanged; recoverable orphan: {}; stdout log: {}; stderr log: {}; {error}",
                        paths.session.display(),
                        paths.stdout_log.display(),
                        paths.stderr_log.display(),
                    );
                })?;
            return write_json_line(&serde_json::json!({
                "alias": arguments.name,
                "predecessor": {
                    "canonical_session": predecessor_entry.canonical_session,
                    "session_id": predecessor_entry.session_id,
                },
                "successor": {
                    "canonical_session": successor.canonical_session,
                    "session_id": successor.session_id,
                    "recorder_pid": successor.recorder_pid,
                    "recorder_process_start_ticks": successor.recorder_process_start_ticks,
                },
                "stdout_log": paths.stdout_log,
                "stderr_log": paths.stderr_log,
                "ready": true,
            }));
        }
        if let Some(status) = recorder.try_wait().into_diagnostic()? {
            return Err(miette::miette!(
                "restart successor exited before readiness with {status}; alias remains bound to predecessor; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "restart successor did not become ready within {} ms; alias remains bound to predecessor; recoverable orphan: {}; stdout log: {}; stderr log: {}",
                arguments.timeout_ms,
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn restart_record_arguments(
    alias: &str,
    session: &Path,
    predecessor: &Path,
    overrides: &str,
) -> Vec<OsString> {
    return vec![
        OsString::from("record"),
        OsString::from("--no-input"),
        OsString::from("--out"),
        session.as_os_str().to_os_string(),
        OsString::from("--session-alias"),
        OsString::from(alias),
        OsString::from("--restart-source"),
        predecessor.as_os_str().to_os_string(),
        OsString::from("--restart-overrides"),
        OsString::from(overrides),
        OsString::from("--"),
        OsString::from("/bin/true"),
    ];
}

fn restarted_record_options(
    output_directory: PathBuf,
    creation_alias: Option<String>,
    predecessor: &Path,
    overrides: &RestartOverrides,
) -> Result<RecordOptions> {
    let manifest_bytes = fs_err::read(predecessor.join("session.json")).into_diagnostic()?;
    let manifest = read_manifest(predecessor)?;
    let completion = wait_for_completion(predecessor, Duration::ZERO)?;
    let event_log = EventLog::read(predecessor.join(&manifest.event_log))?;
    let working_directory = manifest
        .working_directory
        .as_ref()
        .map(|value| return PathBuf::from(OsString::from_vec(value.as_bytes().to_vec())))
        .ok_or_else(|| miette::miette!("predecessor has no effective working directory"))?;
    if manifest.geometry.cols == 0
        || manifest.geometry.rows == 0
        || manifest.geometry.pixel_width % manifest.geometry.cols != 0
        || manifest.geometry.pixel_height % manifest.geometry.rows != 0
    {
        return Err(miette::miette!(
            "predecessor pixel geometry is not an integral cell grid"
        ));
    }
    let ghostty_profile = manifest
        .ghostty_visual
        .as_ref()
        .map(|provenance| {
            return fs_err::read(predecessor.join(&provenance.effective_config))
                .map(|effective_config| {
                    return ResolvedGhosttyProfile {
                        effective_config,
                        provenance: provenance.clone(),
                    };
                })
                .into_diagnostic();
        })
        .transpose()?;
    let term = manifest
        .child_environment
        .get("TERM")
        .cloned()
        .ok_or_else(|| miette::miette!("predecessor manifest has no TERM value"))?;
    let mut options = RecordOptions {
        output_directory,
        creation_alias,
        lineage: Some(SessionLineage {
            predecessor_session_id: manifest.session_id,
            predecessor_manifest_hash: manifest_hash(&manifest_bytes),
            predecessor_final_event_hash: completion.final_event_hash,
            reason: "restart".to_owned(),
        }),
        import_provenance: None,
        command: manifest
            .command
            .iter()
            .map(|value| {
                return OsString::from_vec(value.as_bytes().to_vec());
            })
            .collect(),
        working_directory: Some(working_directory),
        cols: manifest.geometry.cols,
        rows: manifest.geometry.rows,
        cell_width_px: manifest.geometry.pixel_width / manifest.geometry.cols,
        cell_height_px: manifest.geometry.pixel_height / manifest.geometry.rows,
        geometry_provenance: manifest.geometry_provenance,
        ghostty_profile,
        term,
        colors: event_log.header.colors,
        forward_stdin: false,
        durability_policy: manifest.durability_policy,
        provenance_policy: manifest.execution_provenance.policy,
    };
    apply_restart_overrides(&mut options, overrides)?;
    return Ok(options);
}

fn empty_restart_overrides() -> RestartOverrides {
    return RestartOverrides {
        cwd: None,
        cols: None,
        rows: None,
        cell_width: None,
        cell_height: None,
        term: None,
        durability: None,
        durability_max_interval_ms: None,
        durability_max_uncommitted_bytes: None,
        provenance: None,
        provenance_environment: Vec::new(),
        command: None,
    };
}

fn restart_overrides_from_args(arguments: &RestartArgs) -> Result<RestartOverrides> {
    if arguments
        .command
        .first()
        .is_some_and(|value| return value.is_empty())
    {
        return Err(miette::miette!("restart command must not be empty"));
    }
    return Ok(RestartOverrides {
        cwd: arguments
            .cwd
            .as_ref()
            .map(|path| return ByteString::from(path.as_os_str().as_bytes())),
        cols: arguments.cols,
        rows: arguments.rows,
        cell_width: arguments.cell_width,
        cell_height: arguments.cell_height,
        term: arguments.term.clone(),
        durability: arguments.durability,
        durability_max_interval_ms: arguments.durability_max_interval_ms,
        durability_max_uncommitted_bytes: arguments.durability_max_uncommitted_bytes,
        provenance: arguments.provenance,
        provenance_environment: arguments.provenance_environment.clone(),
        command: (!arguments.command.is_empty()).then(|| {
            return arguments
                .command
                .iter()
                .map(|value| return ByteString::from(value.as_os_str().as_bytes()))
                .collect();
        }),
    });
}

fn apply_restart_overrides(
    options: &mut RecordOptions,
    overrides: &RestartOverrides,
) -> Result<()> {
    if let Some(cwd) = &overrides.cwd {
        options.working_directory =
            Some(PathBuf::from(OsString::from_vec(cwd.as_bytes().to_vec())));
    }
    if let Some(command) = &overrides.command {
        options.command = command
            .iter()
            .map(|value| return OsString::from_vec(value.as_bytes().to_vec()))
            .collect();
    }
    options.cols = overrides.cols.unwrap_or(options.cols);
    options.rows = overrides.rows.unwrap_or(options.rows);
    options.cell_width_px = overrides.cell_width.unwrap_or(options.cell_width_px);
    options.cell_height_px = overrides.cell_height.unwrap_or(options.cell_height_px);
    if options.cols == 0
        || options.rows == 0
        || options.cell_width_px == 0
        || options.cell_height_px == 0
    {
        return Err(miette::miette!(
            "restart geometry values must be greater than zero"
        ));
    }
    if overrides.cols.is_some()
        || overrides.rows.is_some()
        || overrides.cell_width.is_some()
        || overrides.cell_height.is_some()
    {
        options.geometry_provenance = GeometryProvenance::default();
    }
    if let Some(term) = &overrides.term {
        options.term.clone_from(term);
    }
    if overrides.durability_max_interval_ms == Some(0)
        || overrides.durability_max_uncommitted_bytes == Some(0)
    {
        return Err(miette::miette!(
            "bounded durability thresholds must be greater than zero"
        ));
    }
    if let Some(mode) = overrides.durability {
        let interval = overrides
            .durability_max_interval_ms
            .or(options.durability_policy.maximum_interval_ms)
            .unwrap_or(50);
        let bytes = overrides
            .durability_max_uncommitted_bytes
            .or(options.durability_policy.maximum_uncommitted_bytes)
            .unwrap_or(65_536);
        options.durability_policy = mode.policy(interval, bytes);
    } else if overrides.durability_max_interval_ms.is_some()
        || overrides.durability_max_uncommitted_bytes.is_some()
    {
        if !matches!(options.durability_policy.mode, DurabilityMode::Bounded) {
            return Err(miette::miette!(
                "durability thresholds require bounded predecessor policy or --durability bounded"
            ));
        }
        if let Some(value) = overrides.durability_max_interval_ms {
            options.durability_policy.maximum_interval_ms = Some(value);
        }
        if let Some(value) = overrides.durability_max_uncommitted_bytes {
            options.durability_policy.maximum_uncommitted_bytes = Some(value);
        }
    }
    if !overrides.provenance_environment.is_empty()
        && !matches!(overrides.provenance, Some(ProvenancePolicyArg::Custom))
    {
        return Err(miette::miette!(
            "--provenance-env requires --provenance custom"
        ));
    }
    if let Some(kind) = overrides.provenance {
        options.provenance_policy = ProvenancePolicy {
            kind: kind.into(),
            environment: overrides.provenance_environment.iter().cloned().collect(),
        };
    }
    return Ok(());
}

fn detached_record_arguments(alias: &str, session: &Path) -> Result<Vec<OsString>> {
    let mut source = env::args_os();
    let _executable = source.next();
    let mut arguments = Vec::new();
    loop {
        let argument = source.next().ok_or_else(|| {
            return miette::miette!("cannot locate start subcommand in process arguments");
        })?;
        if argument == "start" {
            break;
        }
        arguments.push(argument);
    }
    let supplied_name = source
        .next()
        .ok_or_else(|| miette::miette!("start session name is missing"))?;
    if supplied_name != alias {
        return Err(miette::miette!(
            "parsed start name does not match process arguments"
        ));
    }
    arguments.push(OsString::from("record"));
    arguments.push(OsString::from("--no-input"));
    arguments.push(OsString::from("--out"));
    arguments.push(session.as_os_str().to_os_string());
    arguments.push(OsString::from("--session-alias"));
    arguments.push(OsString::from(alias));
    while let Some(argument) = source.next() {
        if argument == "--startup-timeout-ms" {
            let _timeout = source.next().ok_or_else(|| {
                return miette::miette!("--startup-timeout-ms value is missing");
            })?;
            continue;
        }
        if argument
            .as_os_str()
            .as_bytes()
            .starts_with(b"--startup-timeout-ms=")
        {
            continue;
        }
        arguments.push(argument);
    }
    return Ok(arguments);
}

fn open_detached_log(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })
        .map_err(Into::into);
}

fn parse_provenance_rule(value: &str) -> StdResult<(String, ProvenanceCapture), String> {
    let Some((name, capture)) = value.split_once('=') else {
        return Err("expected NAME=value|hash|name|not-captured".to_owned());
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| return byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err("environment name must contain only ASCII letters, digits, or '_'".to_owned());
    }
    let capture = match capture {
        "value" => ProvenanceCapture::Value,
        "hash" => ProvenanceCapture::Hash,
        "name" => ProvenanceCapture::Name,
        "not-captured" => ProvenanceCapture::NotCaptured,
        _ => return Err("capture must be value, hash, name, or not-captured".to_owned()),
    };
    return Ok((name.to_owned(), capture));
}

fn resolve_record_geometry(
    arguments: &RecordingArgs,
    ghostty_profile: Option<&ResolvedGhosttyProfile>,
) -> Result<(SessionGeometry, GeometryProvenance)> {
    match arguments.geometry {
        GeometryMode::Manual => {
            let pixel_width = arguments
                .cols
                .checked_mul(arguments.cell_width)
                .ok_or_else(|| miette::miette!("manual terminal width overflow"))?;
            let pixel_height = arguments
                .rows
                .checked_mul(arguments.cell_height)
                .ok_or_else(|| miette::miette!("manual terminal height overflow"))?;
            return Ok((
                SessionGeometry {
                    cols: arguments.cols,
                    rows: arguments.rows,
                    pixel_width,
                    pixel_height,
                },
                GeometryProvenance {
                    source: GeometrySource::Manual,
                    capture_target: arguments.viewport,
                    ghostty_config_hash: None,
                },
            ));
        }
        GeometryMode::Inherit => {
            let geometry = inherited_geometry(arguments.cell_width, arguments.cell_height)?;
            return Ok((
                geometry,
                GeometryProvenance {
                    source: GeometrySource::Inherited,
                    capture_target: arguments.viewport,
                    ghostty_config_hash: None,
                },
            ));
        }
        GeometryMode::Fit => {
            let target = arguments.viewport.ok_or_else(|| {
                return miette::miette!("--geometry fit requires --viewport WIDTHxHEIGHT");
            })?;
            let profile = ghostty_profile.ok_or_else(|| {
                return miette::miette!("fitted geometry requires a resolved Ghostty profile");
            })?;
            let fitted = probe_ghostty_geometry(target, profile)?;
            let geometry = fitted.report.session_geometry()?;
            return Ok((
                geometry,
                GeometryProvenance {
                    source: GeometrySource::GhosttyFit,
                    capture_target: Some(target),
                    ghostty_config_hash: Some(profile.provenance.effective_config_hash.clone()),
                },
            ));
        }
    }
}

fn parse_capture_target(value: &str) -> StdResult<CaptureTarget, String> {
    let Some((width, height)) = value.split_once(['x', 'X']) else {
        return Err("viewport must use WIDTHxHEIGHT, for example 1920x1080".to_owned());
    };
    let width = width
        .parse::<u16>()
        .map_err(|error| return format!("invalid viewport width: {error}"))?;
    let height = height
        .parse::<u16>()
        .map_err(|error| return format!("invalid viewport height: {error}"))?;
    if width == 0 || height == 0 {
        return Err("viewport dimensions must be non-zero".to_owned());
    }
    return Ok(CaptureTarget { width, height });
}

fn parse_font_size(value: &str) -> StdResult<String, String> {
    let size = value
        .parse::<f32>()
        .map_err(|error| return format!("invalid font size: {error}"))?;
    if !size.is_finite() || size <= 0.0 {
        return Err("font size must be a positive finite number".to_owned());
    }
    return Ok(value.to_owned());
}

fn parse_rgb_color(value: &str) -> StdResult<String, String> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err("color must use #RRGGBB syntax".to_owned());
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| return byte.is_ascii_hexdigit()) {
        return Err("color must use #RRGGBB syntax".to_owned());
    }
    return Ok(format!("#{}", hex.to_ascii_lowercase()));
}
