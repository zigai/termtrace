//! Command-line entry point for Termtrace.

use std::{
    ffi::OsString,
    fmt::Write as _,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    result::Result as StdResult,
    time::Duration,
};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use termtrace::{
    Error,
    control::{CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ProcessSignal, request},
    event::{ByteString, EVENT_LOG_VERSION},
    event_log::EventLog,
    geometry::{inherited_geometry, run_probe},
    ghostty_vt::RowState,
    inspection::{inspect_session, state_text, wait_for_completion, wait_for_output},
    interop::export_asciicast,
    playback::{PlaybackOptions, playback},
    profile::{
        ResolvedGhosttyProfile, resolve_ghostty_profile, resolved_terminal_colors,
        terminal_colors_for_profile,
    },
    record::{RecordOptions, record},
    replay::{
        GHOSTTY_REVISION, IndexOptions, STATE_FORMAT_VERSION, StateSelection, StateSelector,
        index_session_with_options,
    },
    schema::write_all as write_schemas,
    session::{
        CaptureTarget, GeometryProvenance, GeometrySource, GhosttyVisualOverrides, SessionGeometry,
        VerificationLevel, read_manifest, verify_session,
    },
    timeline::{parse_time_offset_ns, resolve_source_selection},
    visual::{CaptureOptions, capture, probe_ghostty_geometry},
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
    Version,
    /// Run a command in a PTY and preserve its byte-exact event stream.
    Record(RecordArgs),
    /// Verify a completed session's structure and evidence bindings offline.
    Verify(VerifyArgs),
    /// Verify that a live session control socket is responsive.
    Ping(SessionArgs),
    /// Print machine-readable live session status.
    Status(StatusArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Resize a live child PTY.
    Resize(ResizeArgs),
    /// Signal a live child process group.
    Signal(SignalArgs),
    /// Add a marker to a live session timeline.
    Mark(MarkArgs),
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

#[derive(Debug, Args)]
struct WaitArgs {
    /// Live session directory.
    #[arg(long)]
    session: PathBuf,
    /// Exact UTF-8 text to observe across output chunks.
    #[arg(long)]
    text: String,
    /// Maximum wait duration in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
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
}

#[derive(Debug, Args)]
struct SessionArgs {
    /// Live session directory containing `control.sock`.
    #[arg(long)]
    session: PathBuf,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Session directory containing a live control socket or completion manifest.
    #[arg(long)]
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
    /// Live session directory containing `control.sock`.
    #[arg(long)]
    session: PathBuf,
    /// Wait for and validate durable completion after stop is acknowledged.
    #[arg(long)]
    wait: bool,
    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("input")
        .required(true)
        .multiple(false)
        .args(["text", "file"])
))]
struct SendArgs {
    /// Live session directory.
    #[arg(long)]
    session: PathBuf,
    /// UTF-8 text to send exactly, without an implicit newline.
    #[arg(long)]
    text: Option<String>,
    /// File whose bytes are sent exactly.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Human-readable reason retained with the input event.
    #[arg(long)]
    intent: Option<String>,
}

#[derive(Debug, Args)]
struct ResizeArgs {
    /// Live session directory.
    #[arg(long)]
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
    /// Live session directory.
    #[arg(long)]
    session: PathBuf,
    /// Allow-listed signal.
    #[arg(long, value_enum)]
    signal: CliSignal,
}

#[derive(Debug, Args)]
struct MarkArgs {
    /// Live session directory.
    #[arg(long)]
    session: PathBuf,
    /// Marker label.
    #[arg(long)]
    label: String,
}

#[derive(Debug, Args)]
struct RecordArgs {
    /// New owner-only session directory.
    #[arg(long)]
    out: PathBuf,

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

    /// Do not forward standard input to the child PTY.
    #[arg(long)]
    no_input: bool,

    /// Executable and arguments, following `--`.
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GeometryMode {
    Manual,
    Inherit,
    Fit,
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
        Command::Version => {
            return write_stdout(format!(
                "termtrace {}\nbuild {}\nlibghostty-vt {GHOSTTY_REVISION}\nevent-log v{EVENT_LOG_VERSION}\nstate-format v{STATE_FORMAT_VERSION}\ncontrol-protocol v{CONTROL_PROTOCOL_VERSION}\n",
                env!("CARGO_PKG_VERSION"),
                env!("TERMTRACE_BUILD_REVISION")
            ));
        }
        Command::Record(arguments) => return run_record(arguments),
        Command::Verify(arguments) => return run_verify(arguments),
        Command::Ping(arguments) => return run_control(arguments.session, ControlCommand::Ping),
        Command::Status(arguments) => return run_status(arguments),
        Command::Send(arguments) => return run_send(arguments),
        Command::Resize(arguments) => return run_resize(arguments),
        Command::Signal(arguments) => {
            return run_control(
                arguments.session,
                ControlCommand::Signal {
                    signal: arguments.signal.into(),
                },
            );
        }
        Command::Mark(arguments) => {
            return run_control(
                arguments.session,
                ControlCommand::Mark {
                    label: arguments.label,
                },
            );
        }
        Command::Stop(arguments) => return run_stop(arguments),
        Command::Index(arguments) => return run_index(arguments),
        Command::Capture(arguments) => return run_capture(arguments),
        Command::Playback(arguments) => return run_playback(arguments),
        Command::GeometryProbe(arguments) => {
            run_probe(&arguments.out)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Inspect(arguments) => return run_inspect(arguments),
        Command::Wait(arguments) => return run_wait(arguments),
        Command::Recover(arguments) => return run_recover(arguments),
        Command::Schemas(arguments) => return run_schemas(arguments),
        Command::ExportAsciicast(arguments) => {
            export_asciicast(arguments.session, arguments.out)?;
            return Ok(ExitCode::SUCCESS);
        }
    }
}

fn run_verify(arguments: VerifyArgs) -> Result<ExitCode> {
    let report = verify_session(arguments.session, arguments.level.into());
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
    let target = match (arguments.width, arguments.height) {
        (Some(width), Some(height)) => CaptureTarget { width, height },
        (None, None) => {
            let manifest = read_manifest(&arguments.session)?;
            manifest
                .geometry_provenance
                .capture_target
                .unwrap_or(CaptureTarget {
                    width: 1280,
                    height: 720,
                })
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
    })?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "video": outcome.video,
        "screenshot": outcome.screenshot,
    }));
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

fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let outcome = wait_for_output(
        arguments.session,
        arguments.text.as_bytes(),
        Duration::from_millis(arguments.timeout_ms),
    )?;
    return write_json_line(&outcome);
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
    let data = request(session, command)?;
    return write_json_line(&data);
}

fn run_status(arguments: StatusArgs) -> Result<ExitCode> {
    let completion_path = arguments.session.join("completion.json");
    if arguments.wait_complete || completion_path.is_file() {
        let completion = wait_for_completion(
            &arguments.session,
            Duration::from_millis(arguments.timeout_ms),
        )?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    return run_control(arguments.session, ControlCommand::Status);
}

fn run_stop(arguments: StopArgs) -> Result<ExitCode> {
    if arguments.session.join("completion.json").is_file() {
        let completion = wait_for_completion(&arguments.session, Duration::ZERO)?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    let acknowledged = request(&arguments.session, ControlCommand::Stop)?;
    if !arguments.wait {
        return write_json_line(&acknowledged);
    }
    let completion = wait_for_completion(
        &arguments.session,
        Duration::from_millis(arguments.timeout_ms),
    )?;
    return write_json_line(&serde_json::json!({
        "kind": "completed",
        "completion": completion,
    }));
}

fn resolve_cli_range(session: &Path, range: &TimeRangeArgs) -> Result<(Option<u64>, Option<u64>)> {
    if range.from_marker.is_none() && range.to_marker.is_none() {
        return Ok((range.start_monotonic_ns, range.end_monotonic_ns));
    }
    let event_path = session.join("events.jsonl");
    let live_event_count = if !session.join("completion.json").is_file() {
        match request(session, ControlCommand::Status) {
            Ok(ControlData::Status { next_event_seq, .. }) => Some(next_event_seq),
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
    let data = match (arguments.text, arguments.file) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(path)) => fs_err::read(path).into_diagnostic()?,
        (Some(_), Some(_)) | (None, None) => {
            return Err(miette::miette!(
                "exactly one of --text or --file is required"
            ));
        }
    };
    return run_control(
        arguments.session,
        ControlCommand::Input {
            data: ByteString::new(data),
            intent: arguments.intent,
        },
    );
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
    return run_control(
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
    );
}

fn run_record(arguments: RecordArgs) -> Result<ExitCode> {
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
        output_directory: arguments.out,
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
        forward_stdin: !arguments.no_input,
    })?;
    writeln!(io::stderr(), "recording: {}", outcome.event_log.display()).into_diagnostic()?;

    match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => return u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => return Ok(ExitCode::FAILURE),
    }
}

fn resolve_record_geometry(
    arguments: &RecordArgs,
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
