use std::collections::BTreeMap;

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
        ResolvedResponseMode::Full => Ok(ExitCode::SUCCESS),
        ResolvedResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResolvedResponseMode::Human => {
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
        ResolvedResponseMode::Agent => {
            let response = if success {
                enforce_success_budget(compact_success(&state), &state.command_type, &state.fields)
            } else {
                Ok(compact_failure(&state, error.as_ref()))
            };
            response
                .and_then(bounded_agent_response)
                .and_then(|output| return direct_stdout(&output))
        }
    };
    if let Some(error) = error
        && !matches!(
            state.mode,
            ResolvedResponseMode::Quiet | ResolvedResponseMode::Human
        )
    {
        direct_stderr(&report_message(&error));
    }
    if let Err(error) = write_result {
        direct_stderr(&error.to_string());
        return ExitCode::FAILURE;
    }
    return code;
}

pub(in crate::cli) fn write_clap_error(error: clap::Error, mode: ResponseMode) -> ExitCode {
    let success = matches!(
        error.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    );
    let code = if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2))
    };
    let resolved_mode = if success && mode == ResponseMode::Auto {
        ResolvedResponseMode::Human
    } else {
        mode.resolve()
    };
    match resolved_mode {
        ResolvedResponseMode::Full | ResolvedResponseMode::Human => {
            let _ = error.print();
        }
        ResolvedResponseMode::Quiet => {}
        ResolvedResponseMode::Agent => {
            let response = if success {
                typed_envelope(CliResponseEnvelope {
                    version: CLI_RESPONSE_VERSION,
                    ok: true,
                    response_type: "help".to_owned(),
                    code: None,
                    applied: None,
                    retry: None,
                    session_state: None,
                    request_id: None,
                    message: None,
                    truncated: None,
                    response_bytes: None,
                    fields: BTreeMap::from([(
                        "output".to_owned(),
                        serde_json::Value::String(error.to_string()),
                    )]),
                })
            } else {
                typed_envelope(CliResponseEnvelope {
                    version: CLI_RESPONSE_VERSION,
                    ok: false,
                    response_type: "arguments".to_owned(),
                    code: Some("invalid_arguments".to_owned()),
                    applied: Some(Applied::None),
                    retry: Some(Retry::SafeNewRequest),
                    session_state: Some(SessionState::Unknown),
                    request_id: None,
                    message: Some(error.to_string().trim().to_owned()),
                    truncated: None,
                    response_bytes: None,
                    fields: BTreeMap::new(),
                })
            };
            if let Ok(output) = bounded_agent_response(response) {
                let _ = direct_stdout(&output);
            }
            if !success {
                direct_stderr(error.to_string().trim());
            }
        }
    }
    return code;
}
pub(in crate::cli) struct ResponseOutput<'response> {
    response: &'response mut ResponseWriter,
}

impl io::Write for ResponseOutput<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        return match self.response.mode() {
            ResolvedResponseMode::Full => io::stdout().lock().write(buf),
            ResolvedResponseMode::Quiet => Ok(buf.len()),
            ResolvedResponseMode::Human | ResolvedResponseMode::Agent => {
                capture_raw(self.response, buf);
                Ok(buf.len())
            }
        };
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.response.mode() == ResolvedResponseMode::Full {
            return io::stdout().lock().flush();
        }
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
    return match response.mode() {
        ResolvedResponseMode::Full => {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            match stdout
                .write_all(output)
                .and_then(|()| return stdout.flush())
            {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
                Err(error) => Err(error).into_diagnostic(),
            }
        }
        ResolvedResponseMode::Quiet => Ok(true),
        ResolvedResponseMode::Human | ResolvedResponseMode::Agent => {
            capture_raw(response, output);
            Ok(true)
        }
    };
}

pub(in crate::cli) fn write_follow_json_line(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<bool> {
    if response.mode() == ResolvedResponseMode::Full {
        let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
        encoded.push(b'\n');
        return write_follow_stdout(response, &encoded);
    }
    if response.mode() != ResolvedResponseMode::Quiet {
        capture_json(response, value)?;
    }
    return Ok(true);
}

pub(in crate::cli) fn write_json_line(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<ExitCode> {
    return match response.mode() {
        ResolvedResponseMode::Full => {
            let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
            encoded.push(b'\n');
            direct_stdout(&encoded)
        }
        ResolvedResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResolvedResponseMode::Human | ResolvedResponseMode::Agent => {
            capture_json(response, value)?;
            Ok(ExitCode::SUCCESS)
        }
    };
}

pub(in crate::cli) fn write_stdout(
    response: &mut ResponseWriter,
    output: impl AsRef<[u8]>,
) -> Result<ExitCode> {
    return match response.mode() {
        ResolvedResponseMode::Full => direct_stdout(output.as_ref()),
        ResolvedResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResolvedResponseMode::Human | ResolvedResponseMode::Agent => {
            capture_raw(response, output.as_ref());
            Ok(ExitCode::SUCCESS)
        }
    };
}
