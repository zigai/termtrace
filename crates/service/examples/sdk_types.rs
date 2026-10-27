//! Print the SDK type catalog as JSON for the binding type generators.

use std::{
    io::{stderr, stdout, Write as _},
    process::ExitCode,
};
use termtrace_service::sdk_contract::types::catalog;

fn main() -> ExitCode {
    let catalog = match catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            let _ignored = writeln!(stderr(), "{error}");

            return ExitCode::FAILURE;
        }
    };

    let mut stdout = stdout().lock();
    if serde_json::to_writer_pretty(&mut stdout, &catalog).is_err() || writeln!(stdout).is_err() {
        return ExitCode::FAILURE;
    }
    return ExitCode::SUCCESS;
}
