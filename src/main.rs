//! Command-line entry point for Termtrace.

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand};
use miette::{IntoDiagnostic as _, Result};
use termtrace::record::{RecordOptions, record};
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

    match cli.command {
        Command::Version => {
            println!("termtrace {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Command::Record(arguments) => run_record(arguments),
    }
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
    })?;
    eprintln!("recording: {}", outcome.event_log.display());

    match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => Ok(ExitCode::FAILURE),
    }
}
