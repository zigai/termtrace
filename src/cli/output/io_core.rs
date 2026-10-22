use super::*;

pub(in crate::cli) fn write_progress(response: &ResponseWriter, message: impl Display) {
    if response.mode() == ResponseMode::Quiet {
        return;
    }

    // Progress is advisory, and standard error has no fallback channel to report its own failure.
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    drop(writeln!(stderr, "{message}"));
}

pub(super) fn direct_stdout(output: &[u8]) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}

pub(super) fn direct_stderr(message: &str) {
    // Standard error is the last-resort diagnostic channel; its own failure has nowhere to go.
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    drop(writeln!(stderr, "termtrace: {message}"));
}

pub(in crate::cli) fn report_message(error: &miette::Report) -> String {
    let mut message = error.to_string();

    if let Some(help) = error.help() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "`fmt::Write` for `String` never fails, as documented on `format!`, and `drop_non_drop` rejects dropping the unit result"
        )]
        let _ = write!(message, "\nhelp: {help}");
    }

    return message;
}
