//! Command-line entry point for Termtrace.

use clap::FromArgMatches as _;
use std::{io, process::ExitCode};
use tracing_subscriber::{
    fmt, layer::SubscriberExt, registry as tracing_registry, util::SubscriberInitExt, EnvFilter,
};

mod agent;
mod analysis;
mod artifacts;
mod command;
mod control;
mod dispatch;
mod integrity;
mod output;
mod owned;
mod parsers;
mod recording;
mod registry;
mod scenario;
mod session_helpers;
mod tmux;
mod tooling;

use command::{requested_quiet_mode, root, Cli};
use output::{finish_response, write_clap_error, ResponseMode, ResponseWriter};

fn init_diagnostics(level: tracing::Level) {
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    let layer = fmt::layer()
        .with_target(false)
        .with_writer(io::stderr)
        .with_ansi(!command::should_disable_color());

    tracing_registry()
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .with(layer)
        .init();
}

pub(crate) fn run() -> ExitCode {
    let quiet = requested_quiet_mode();
    let matches = match root().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => return write_clap_error(error, quiet),
    };

    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => return write_clap_error(error, quiet),
    };

    if let Err(error) = cli.validate() {
        return write_clap_error(error, quiet);
    }

    let response_mode = if cli.is_quiet() { ResponseMode::Quiet } else { ResponseMode::Human };
    let mut response = ResponseWriter::new(response_mode);

    if !cli.is_quiet() {
        init_diagnostics(cli.log);
    }

    let result = dispatch::dispatch(&mut response, cli.command);

    return finish_response(response, result);
}
