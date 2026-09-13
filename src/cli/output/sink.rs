use super::*;

pub(in crate::cli) fn finish_response(
    response: ResponseWriter,
    result: Result<ExitCode>,
) -> ExitCode {
    let state = response.state;
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(error) => (ExitCode::FAILURE, Some(error)),
    };
    let success = code == ExitCode::SUCCESS && error.is_none();

    let write_result = match state.mode {
        ResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResponseMode::Human => {
            if success {
                if !state.raw.is_empty() {
                    direct_stdout(&state.raw)
                } else {
                    let summary = state
                        .values
                        .iter()
                        .map(|value| return human_summary(value, &state.command_type))
                        .collect::<String>();

                    direct_stdout(summary.as_bytes())
                }
            } else {
                direct_stderr(&human_failure_summary(&state, error.as_ref()));

                Ok(ExitCode::SUCCESS)
            }
        }
    };

    if let Some(error) = error
        && state.mode != ResponseMode::Quiet
    {
        direct_stderr(&report_message(&error));
    }

    if let Err(error) = write_result {
        direct_stderr(&error.to_string());

        return ExitCode::FAILURE;
    }
    return code;
}

pub(in crate::cli) fn write_clap_error(error: clap::Error, quiet: bool) -> ExitCode {
    let success = matches!(
        error.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    );

    let code = if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2))
    };

    if success {
        let _ = error.print();
    } else if !quiet {
        let err_str = error.to_string();
        let trimmed = err_str.trim_end();
        direct_stderr(&format!("{trimmed}\n"));
    }

    return code;
}

pub(in crate::cli) struct ResponseOutput<'response> {
    response: &'response mut ResponseWriter,
}

impl io::Write for ResponseOutput<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        return match self.response.mode() {
            ResponseMode::Quiet => Ok(buf.len()),
            ResponseMode::Human => {
                let stdout = io::stdout();
                let mut stdout = stdout.lock();
                stdout.write_all(buf)?;
                stdout.flush()?;

                Ok(buf.len())
            }
        };
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

pub(in crate::cli) fn response_output(response: &mut ResponseWriter) -> ResponseOutput<'_> {
    return ResponseOutput { response };
}

pub(in crate::cli) fn write_follow_stdout(
    response: &mut ResponseWriter,
    output: &[u8],
) -> Result<bool> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(true);
    }

    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    return match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error).into_diagnostic(),
    };
}

pub(in crate::cli) fn write_follow_json_line(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<bool> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(true);
    }

    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');

    return write_follow_stdout(response, &encoded);
}

pub(in crate::cli) fn write_json_line(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<ExitCode> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(ExitCode::SUCCESS);
    }

    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');

    return direct_stdout(&encoded);
}

pub(in crate::cli) fn write_json_pretty(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<ExitCode> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(ExitCode::SUCCESS);
    }

    let mut encoded = serde_json::to_vec_pretty(value).into_diagnostic()?;
    encoded.push(b'\n');

    return direct_stdout(&encoded);
}

pub(in crate::cli) fn write_stdout(
    response: &mut ResponseWriter,
    output: impl AsRef<[u8]>,
) -> Result<ExitCode> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(ExitCode::SUCCESS);
    }
    return direct_stdout(output.as_ref());
}
