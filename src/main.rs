//! Termtrace command-line entrypoint.

mod cli;

use std::process::ExitCode;

fn main() -> miette::Result<ExitCode> {
    return cli::run();
}
