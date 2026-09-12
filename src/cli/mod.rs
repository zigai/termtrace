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

use command::{
    command_contract, preserves_requested_output, requested_response_mode, root_command, Cli,
};
use output::{
    finish_response, write_clap_error, ResolvedResponseMode, ResponseMode, ResponseWriter,
};

fn init_diagnostics(level: tracing::Level) {
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    tracing_registry()
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .with(fmt::layer().with_target(false).with_writer(io::stderr))
        .init();
}

pub(crate) fn run() -> ExitCode {
    let requested_mode = requested_response_mode();
    let matches = match root_command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => return write_clap_error(error, requested_mode),
    };

    let (command_type, mutating) = command_contract(&matches);
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => return write_clap_error(error, requested_mode),
    };

    let response_mode =
        if cli.response == ResponseMode::Auto && preserves_requested_output(&cli.command) {
            ResolvedResponseMode::Full
        } else {
            cli.response.resolve()
        };

    let mut response = ResponseWriter::new(response_mode, cli.fields, command_type, mutating);

    if response_mode != ResolvedResponseMode::Quiet {
        init_diagnostics(cli.log);
    }

    let result = dispatch::dispatch(&mut response, cli.command);

    return finish_response(response, result);
}
