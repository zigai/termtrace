//! Command-line entry point for Termtrace.

use std::{
    ffi::OsString,
    io::{self, Write as _},
    path::PathBuf,
    process::ExitCode,
    result::Result as StdResult,
    time::Duration,
};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use termtrace::{
    control::{CONTROL_PROTOCOL_VERSION, ControlCommand, ProcessSignal, request},
    event::{ByteString, EVENT_LOG_VERSION},
    event_log::EventLog,
    geometry::{inherited_geometry, run_probe},
    inspection::{inspect_session, state_text, wait_for_output},
    interop::export_asciicast,
    playback::{PlaybackOptions, playback},
    profile::{
        ResolvedGhosttyProfile, resolve_ghostty_profile, resolved_terminal_colors,
        terminal_colors_for_profile,
    },
    record::{RecordOptions, record},
    replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION, StateSelector, index_session_range},
    schema::write_all as write_schemas,
    session::{
        CaptureTarget, GeometryProvenance, GeometrySource, GhosttyVisualOverrides, SessionGeometry,
        read_manifest,
    },
    timeline::parse_time_offset_ns,
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
    /// Verify that a live session control socket is responsive.
    Ping(SessionArgs),
    /// Print machine-readable live session status.
    Status(SessionArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Resize a live child PTY.
    Resize(ResizeArgs),
    /// Signal a live child process group.
    Signal(SignalArgs),
    /// Add a marker to a live session timeline.
    Mark(MarkArgs),
    /// Request graceful termination of a live session child.
    Stop(SessionArgs),
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
    /// Capture width; defaults to the recorded viewport or 1280 pixels.
    #[arg(long, requires = "height")]
    width: Option<u16>,
    /// Capture height; defaults to the recorded viewport or 720 pixels.
    #[arg(long, requires = "width")]
    height: Option<u16>,
    /// Video frame rate; native capture currently requires 24.
    #[arg(long, default_value_t = 24)]
    fps: u16,
    #[command(flatten)]
    range: TimeRangeArgs,
}

#[derive(Clone, Copy, Debug, Default, Args)]
struct TimeRangeArgs {
    /// Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`.
    #[arg(long = "from", value_name = "OFFSET", value_parser = parse_time_offset_ns)]
    start_monotonic_ns: Option<u64>,
    /// Exclusive source offset, in the same format as `--from`.
    #[arg(long = "to", value_name = "OFFSET", value_parser = parse_time_offset_ns)]
    end_monotonic_ns: Option<u64>,
}

#[derive(Debug, Args)]
struct IndexArgs {
    /// Source session directory.
    #[arg(long)]
    session: PathBuf,
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
            println!("termtrace {}", env!("CARGO_PKG_VERSION"));
            println!("libghostty-vt {GHOSTTY_REVISION}");
            println!("event-log v{EVENT_LOG_VERSION}");
            println!("state-format v{STATE_FORMAT_VERSION}");
            println!("control-protocol v{CONTROL_PROTOCOL_VERSION}");
            return Ok(ExitCode::SUCCESS);
        }
        Command::Record(arguments) => return run_record(arguments),
        Command::Ping(arguments) => return run_control(arguments.session, ControlCommand::Ping),
        Command::Status(arguments) => {
            return run_control(arguments.session, ControlCommand::Status);
        }
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
        Command::Stop(arguments) => return run_control(arguments.session, ControlCommand::Stop),
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

fn run_index(arguments: IndexArgs) -> Result<ExitCode> {
    let outcome = index_session_range(
        arguments.session,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "directory": outcome.directory,
            "state_count": outcome.state_count,
            "final_state_hash": outcome.final_state_hash,
        })
    );
    return Ok(ExitCode::SUCCESS);
}

fn run_capture(arguments: CaptureArgs) -> Result<ExitCode> {
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
        start_monotonic_ns: arguments.range.start_monotonic_ns,
        end_monotonic_ns: arguments.range.end_monotonic_ns,
    })?;
    println!(
        "{}",
        serde_json::json!({
            "directory": outcome.directory,
            "video": outcome.video,
            "screenshot": outcome.screenshot,
        })
    );
    return Ok(ExitCode::SUCCESS);
}

fn run_playback(arguments: PlaybackArgs) -> Result<ExitCode> {
    playback(&PlaybackOptions {
        session_directory: arguments.session,
        timing_file: arguments.timing_file,
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
    match arguments.format {
        InspectFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&inspection).into_diagnostic()?
        ),
        InspectFormat::Text => println!("{}", state_text(&inspection.state)),
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let outcome = wait_for_output(
        arguments.session,
        arguments.text.as_bytes(),
        Duration::from_millis(arguments.timeout_ms),
    )?;
    println!("{}", serde_json::to_string(&outcome).into_diagnostic()?);
    return Ok(ExitCode::SUCCESS);
}

fn run_recover(arguments: RecoverArgs) -> Result<ExitCode> {
    let report = EventLog::recover_prefix(arguments.event_log, arguments.out)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).into_diagnostic()?
    );
    return Ok(ExitCode::SUCCESS);
}

fn run_schemas(arguments: SchemaArgs) -> Result<ExitCode> {
    for path in write_schemas(arguments.out)? {
        println!("{}", path.display());
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_control(session: PathBuf, command: ControlCommand) -> Result<ExitCode> {
    let data = request(session, command)?;
    println!("{}", serde_json::to_string(&data).into_diagnostic()?);
    return Ok(ExitCode::SUCCESS);
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
