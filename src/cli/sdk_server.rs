//! JSON-RPC server that SDK clients in other languages start as a child process.

use miette::{IntoDiagnostic as _, Result};
use std::{io, process::ExitCode};

pub(in crate::cli) fn run_sdk_server() -> Result<ExitCode> {
    termtrace_sdk_server::serve(io::stdin().lock(), io::stdout()).into_diagnostic()?;

    return Ok(ExitCode::SUCCESS);
}
