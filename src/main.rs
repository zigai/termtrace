//! Command-line entry point for Termtrace.

use std::{ffi::OsString, path::PathBuf, process::ExitCode, time::Duration};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use termtrace::{
    control::{ControlCommand, ProcessSignal, request},
    event::ByteString,
    event_log::EventLog,
    inspection::{inspect_session, state_text, wait_for_output},
    interop::export_asciicast,
    playback::{PlaybackOptions, playback},
    record::{RecordOptions, record},
    replay::{StateSelector, index_session},
    schema::write_all as write_schemas,
    visual::{CaptureOptions, capture},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
    Index(SessionArgs),
    /// Capture actual Ghostty pixels on a private headless display.
    Capture(CaptureArgs),
    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true)]
    Playback(PlaybackArgs),
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
    /// Capture width in pixels.
    #[arg(long, default_value_t = 1280)]
    width: u16,
    /// Capture height in pixels.
    #[arg(long, default_value_t = 720)]
    height: u16,
    /// Video frame rate; native capture currently requires 24.
    #[arg(long, default_value_t = 24)]
    fps: u16,
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
            CliSignal::Interrupt => Self::Interrupt,
            CliSignal::Terminate => Self::Terminate,
            CliSignal::Hangup => Self::Hangup,
            CliSignal::User1 => Self::User1,
            CliSignal::User2 => Self::User2,
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

    /// Child working directory.
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Terminal columns.
    #[arg(long, default_value_t = 160)]
    cols: u16,

    /// Terminal rows.
    #[arg(long, default_value_t = 40)]
    rows: u16,

    /// Cell width reported to the child, in pixels.
    #[arg(long, default_value_t = 9)]
    cell_width: u16,

    /// Cell height reported to the child, in pixels.
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

fn init_diagnostics(level: tracing::Level) {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    init_diagnostics(cli.log);
    dispatch(cli.command)
}

fn dispatch(command: Command) -> Result<ExitCode> {
    match command {
        Command::Version => {
            println!("termtrace {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Command::Record(arguments) => run_record(arguments),
        Command::Ping(arguments) => run_control(arguments.session, ControlCommand::Ping),
        Command::Status(arguments) => run_control(arguments.session, ControlCommand::Status),
        Command::Send(arguments) => run_send(arguments),
        Command::Resize(arguments) => run_resize(arguments),
        Command::Signal(arguments) => run_control(
            arguments.session,
            ControlCommand::Signal {
                signal: arguments.signal.into(),
            },
        ),
        Command::Mark(arguments) => run_control(
            arguments.session,
            ControlCommand::Mark {
                label: arguments.label,
            },
        ),
        Command::Stop(arguments) => run_control(arguments.session, ControlCommand::Stop),
        Command::Index(arguments) => run_index(arguments),
        Command::Capture(arguments) => run_capture(arguments),
        Command::Playback(arguments) => run_playback(arguments),
        Command::Inspect(arguments) => run_inspect(arguments),
        Command::Wait(arguments) => run_wait(arguments),
        Command::Recover(arguments) => run_recover(arguments),
        Command::Schemas(arguments) => run_schemas(arguments),
        Command::ExportAsciicast(arguments) => {
            export_asciicast(arguments.session, arguments.out)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn run_index(arguments: SessionArgs) -> Result<ExitCode> {
    let outcome = index_session(arguments.session)?;
    println!(
        "{}",
        serde_json::json!({
            "directory": outcome.directory,
            "state_count": outcome.state_count,
            "final_state_hash": outcome.final_state_hash,
        })
    );
    Ok(ExitCode::SUCCESS)
}

fn run_capture(arguments: CaptureArgs) -> Result<ExitCode> {
    let outcome = capture(&CaptureOptions {
        session_directory: arguments.session,
        width: arguments.width,
        height: arguments.height,
        fps: arguments.fps,
    })?;
    println!(
        "{}",
        serde_json::json!({
            "directory": outcome.directory,
            "video": outcome.video,
            "screenshot": outcome.screenshot,
        })
    );
    Ok(ExitCode::SUCCESS)
}

fn run_playback(arguments: PlaybackArgs) -> Result<ExitCode> {
    playback(&PlaybackOptions {
        session_directory: arguments.session,
        timing_file: arguments.timing_file,
        ready_file: arguments.ready_file,
        start_delay: Duration::from_millis(arguments.start_delay_ms),
        final_hold: Duration::from_millis(arguments.final_hold_ms),
    })?;
    Ok(ExitCode::SUCCESS)
}

fn run_inspect(arguments: InspectArgs) -> Result<ExitCode> {
    let selector = match (arguments.state, arguments.event_seq, arguments.monotonic_ns) {
        (Some(state), None, None) => StateSelector::StateId(state),
        (None, Some(sequence), None) => StateSelector::EventSeq(sequence),
        (None, None, Some(time)) => StateSelector::MonotonicNs(time),
        (None, None, None) => StateSelector::Final,
        _ => unreachable!("clap validates selector exclusivity"),
    };
    let inspection = inspect_session(arguments.session, selector)?;
    match arguments.format {
        InspectFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&inspection).into_diagnostic()?
        ),
        InspectFormat::Text => println!("{}", state_text(&inspection.state)),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let outcome = wait_for_output(
        arguments.session,
        arguments.text.as_bytes(),
        Duration::from_millis(arguments.timeout_ms),
    )?;
    println!("{}", serde_json::to_string(&outcome).into_diagnostic()?);
    Ok(ExitCode::SUCCESS)
}

fn run_recover(arguments: RecoverArgs) -> Result<ExitCode> {
    let report = EventLog::recover_prefix(arguments.event_log, arguments.out)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).into_diagnostic()?
    );
    Ok(ExitCode::SUCCESS)
}

fn run_schemas(arguments: SchemaArgs) -> Result<ExitCode> {
    for path in write_schemas(arguments.out)? {
        println!("{}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn run_control(session: PathBuf, command: ControlCommand) -> Result<ExitCode> {
    let data = request(session, command)?;
    println!("{}", serde_json::to_string(&data).into_diagnostic()?);
    Ok(ExitCode::SUCCESS)
}

fn run_send(arguments: SendArgs) -> Result<ExitCode> {
    let data = match (arguments.text, arguments.file) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(path)) => fs_err::read(path).into_diagnostic()?,
        (Some(_), Some(_)) | (None, None) => unreachable!("clap validates the input group"),
    };
    run_control(
        arguments.session,
        ControlCommand::Input {
            data: ByteString::new(data),
            intent: arguments.intent,
        },
    )
}

fn run_resize(arguments: ResizeArgs) -> Result<ExitCode> {
    let pixel_width = arguments
        .pixel_width
        .or_else(|| arguments.cols.checked_mul(9))
        .ok_or_else(|| miette::miette!("pixel width overflow; pass --pixel-width explicitly"))?;
    let pixel_height = arguments
        .pixel_height
        .or_else(|| arguments.rows.checked_mul(18))
        .ok_or_else(|| miette::miette!("pixel height overflow; pass --pixel-height explicitly"))?;
    run_control(
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
    )
}

fn run_record(arguments: RecordArgs) -> Result<ExitCode> {
    let outcome = record(&RecordOptions {
        output_directory: arguments.out,
        command: arguments.command,
        working_directory: arguments.cwd,
        cols: arguments.cols,
        rows: arguments.rows,
        cell_width_px: arguments.cell_width,
        cell_height_px: arguments.cell_height,
        term: arguments.term,
        forward_stdin: !arguments.no_input,
    })?;
    eprintln!("recording: {}", outcome.event_log.display());

    match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => Ok(ExitCode::FAILURE),
    }
}
