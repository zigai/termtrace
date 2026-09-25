use super::*;

pub(in crate::cli) fn write_progress(response: &ResponseWriter, message: impl Display) {
    if response.mode() == ResponseMode::Quiet {
        return;
    }

    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "{message}");
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
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "termtrace: {message}");
}

pub(super) fn report_message(error: &miette::Report) -> String {
    let mut message = error.to_string();

    if let Some(help) = error.help() {
        let _ = write!(message, "\nhelp: {help}");
    }

    return message;
}
