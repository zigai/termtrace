//! Termtrace command-line entrypoint.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    return cli::run();
}
