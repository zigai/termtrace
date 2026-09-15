use super::*;
use std::{ffi::OsString, fmt::Write as _, os::unix::ffi::OsStringExt as _};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::protocol::control::ClockSample;

pub(super) fn compact_control_data(data: &ControlData) -> Result<serde_json::Value> {
    let value = match data {
        ControlData::Acknowledged { boundaries } => serde_json::json!({
            "kind": "acknowledged",
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputApplied {
            operation_id,
            input_kind,
            bytes_written,
            intent_event_seq,
            result_event_seq,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "input_applied",
            "operation_id": operation_id,
            "input_kind": input_kind,
            "bytes_written": bytes_written,
            "intent_event_seq": intent_event_seq,
            "result_event_seq": result_event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputSequenceApplied {
            sequence_id,
            atom_count,
            bytes_written,
            first_intent_event_seq,
            last_result_event_seq,
            pace_ns,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "input_sequence_applied",
            "sequence_id": sequence_id,
            "atom_count": atom_count,
            "bytes_written": bytes_written,
            "first_intent_event_seq": first_intent_event_seq,
            "last_result_event_seq": last_result_event_seq,
            "pace_ns": pace_ns,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputNoop {
            input_kind,
            reason,
            terminal_modes_hash,
            boundaries,
        } => serde_json::json!({
            "kind": "input_noop",
            "input_kind": input_kind,
            "reason": reason,
            "terminal_modes_hash": terminal_modes_hash,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::Activated {
            operation_id,
            text,
            start_column,
            end_column,
            row,
            target_column,
            presented_state_hash,
            bytes_written,
            intent_event_seq,
            result_event_seq,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "activated",
            "operation_id": operation_id,
            "text": text,
            "region": {
                "start_column": start_column,
                "end_column": end_column,
                "row": row,
                "target_column": target_column,
            },
            "presented_state_hash": presented_state_hash,
            "bytes_written": bytes_written,
            "intent_event_seq": intent_event_seq,
            "result_event_seq": result_event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::Status {
            pid,
            boundaries,
            clock_sample,
        } => serde_json::json!({
            "kind": "status",
            "pid": pid,
            "boundary": compact_boundaries(boundaries),
            "clock_sample": clock_sample,
        }),
        ControlData::Snapshot {
            viewport_text,
            state_hash,
            presented,
            cols,
            rows,
            cursor_column,
            cursor_row,
            cursor_visible,
            observed_monotonic_ns,
            modes,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "snapshot",
            "state_hash": state_hash,
            "viewport_text": viewport_text,
            "presented": presented,
            "geometry": { "cols": cols, "rows": rows },
            "cursor": { "column": cursor_column, "row": cursor_row, "visible": cursor_visible },
            "observed_monotonic_ns": observed_monotonic_ns,
            "modes": modes,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::ActionApplied { .. }
        | ControlData::EventCursor { .. }
        | ControlData::ScreenFrame { .. }
        | ControlData::StableSnapshot { .. }
        | ControlData::ScreenWait { .. } => serde_json::to_value(data).into_diagnostic()?,
        ControlData::Marked {
            label,
            event_seq,
            boundaries,
        } => serde_json::json!({
            "kind": "marked",
            "label": label,
            "event_seq": event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
    };

    return Ok(value);
}

pub(super) enum CliControlOutcome {
    Applied(Box<ControlData>),
    Rejected(ExitCode),
}

pub(super) fn send_cli_control(
    response: &mut ResponseWriter,
    session: &Path,
    request: &ControlRequest,
    operation: &'static str,
) -> Result<CliControlOutcome> {
    let control_response = send_request_response(session, request).into_diagnostic()?;
    match control_response.result {
        ControlResult::Ok { data } => return Ok(CliControlOutcome::Applied(Box::new(data))),
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "control_rejected",
                    "operation": operation,
                    "code": code,
                    "message": message,
                    "details": details,
                    "request_id": control_response.request_id,
                    "applied": "none",
                    "retry": "safe_new_request",
                    "session_state": "live",
                }),
            )?;

            return Ok(CliControlOutcome::Rejected(ExitCode::FAILURE));
        }
    }
}

#[expect(dead_code, reason = "Control execution helper preserved for tests")]
pub(in crate::cli) fn run_control_guarded(
    response: &mut ResponseWriter,
    session: PathBuf,
    command: ControlCommand,
    expected_presented_state_hash: Option<String>,
    request_id: Option<uuid::Uuid>,
    json: bool,
) -> Result<ExitCode> {
    let resolved = resolve_session(session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "control", ExitCode::FAILURE);
    }

    let mut control_request = ControlRequest::new(command);

    if let Some(request_id) = request_id {
        control_request.request_id = request_id;
    }

    control_request.expected_presented_state_hash = expected_presented_state_hash;
    let data = match send_cli_control(response, &resolved.path, &control_request, "control")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };

    let compacted = compact_control_data(&data)?;
    if json {
        return write_json_pretty(response, &compacted);
    }
    return write_stdout(response, "Control command applied\n");
}

pub(in crate::cli) fn run_status(
    response: &mut ResponseWriter,
    arguments: StatusArgs,
) -> Result<ExitCode> {
    if arguments.gc {
        let registry = SessionRegistry::open_default().into_diagnostic()?;
        let report = registry
            .garbage_collect_with_quota(
                arguments.older_than_days.unwrap_or(30),
                arguments.keep_latest.unwrap_or(20),
                None,
                true,
            )
            .into_diagnostic()?;

        if arguments.json {
            return write_json_pretty(response, &report);
        }
        return write_stdout(
            response,
            format!(
                "Garbage collection completed: {} session(s) removed\n",
                report.removed_sessions,
            ),
        );
    }

    if let Some(alias) = &arguments.rm {
        let registry = SessionRegistry::open_default().into_diagnostic()?;
        let entry = registry.remove(alias).into_diagnostic()?;
        let canonical_session_display = entry.session_path().display().to_string();

        if arguments.json {
            return write_json_pretty(
                response,
                &serde_json::json!({
                    "alias": entry.alias,
                    "canonical_session": entry.canonical_session,
                    "canonical_session_display": canonical_session_display,
                    "session_id": entry.session_id,
                    "evidence_preserved": true,
                }),
            );
        }
        return write_stdout(
            response,
            format!("Removed alias '{alias}' for session {canonical_session_display}\n"),
        );
    }

    let Some(requested_session) = arguments.session else {
        let registry = SessionRegistry::open_default().into_diagnostic()?;
        let entries = registry.list(false).into_diagnostic()?;

        if arguments.json {
            return write_json_pretty(response, &entries);
        }

        if entries.is_empty() {
            return write_stdout(response, "No sessions found.\n");
        }

        let rows = entries
            .into_iter()
            .map(|entry| {
                let path = PathBuf::from(OsString::from_vec(
                    entry.canonical_session.as_bytes().to_vec(),
                ));

                return (
                    entry.alias,
                    entry.lifecycle.as_str().to_owned(),
                    entry.session_id.to_string(),
                    path.display().to_string(),
                );
            })
            .collect::<Vec<_>>();

        let alias_width = rows
            .iter()
            .fold("ALIAS".len(), |width, row| return width.max(row.0.len()));
        let lifecycle_width = rows.iter().fold("LIFECYCLE".len(), |width, row| {
            return width.max(row.1.len());
        });
        let session_id_width = rows.iter().fold("SESSION ID".len(), |width, row| {
            return width.max(row.2.len());
        });

        let mut output = String::new();

        let header = format!(
            "{:<alias_width$}  {:<lifecycle_width$}  {:<session_id_width$}  CANONICAL SESSION",
            "ALIAS", "LIFECYCLE", "SESSION ID",
        );

        let rule = "─".repeat(header.chars().count());

        writeln!(output, "{header}").into_diagnostic()?;
        writeln!(output, "{rule}").into_diagnostic()?;
        for (alias, lifecycle, session_id, path) in rows {
            writeln!(
                output,
                "{alias:<alias_width$}  {lifecycle:<lifecycle_width$}  {session_id:<session_id_width$}  {path}",
            )
            .into_diagnostic()?;
        }

        return write_stdout(response, output);
    };
    let session_label = requested_session.display().to_string();
    let resolved = resolve_session(requested_session)?;
    if arguments.wait_complete || resolved.lifecycle == RegistryLifecycle::Completed {
        let completion =
            wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))
                .into_diagnostic()?;

        let payload = serde_json::json!({
            "kind": "completed",
            "session": session_label,
            "lifecycle": "completed",
            "completion": completion,
        });

        if arguments.json {
            return write_json_pretty(response, &payload);
        }

        let exit_str = completion
            .exit_code
            .map_or_else(|| return "unknown".to_owned(), |c| return c.to_string());

        return write_stdout(
            response,
            format!("Session {session_label} completed (exit code {exit_str})\n"),
        );
    }

    if resolved.lifecycle != RegistryLifecycle::Live {
        if arguments.json {
            return write_session_lifecycle(response, &resolved, "status", ExitCode::SUCCESS);
        }
        return write_stdout(
            response,
            format!("Session {session_label} {}\n", resolved.lifecycle.as_str()),
        );
    }

    let data = request(resolved.path, ControlCommand::Status).into_diagnostic()?;
    if let ControlData::Status {
        pid,
        boundaries,
        clock_sample,
    } = data
    {
        let status = LiveStatusResponse {
            kind: "status",
            session: &session_label,
            lifecycle: "live",
            session_state: "live",
            pid,
            boundary: compact_boundaries(&boundaries),
            clock_sample,
        };

        if arguments.json {
            return write_json_pretty(response, &status);
        }

        let pid_str = pid.map_or_else(|| return "none".to_owned(), |p| return p.to_string());

        return write_stdout(
            response,
            format!(
                "Session {session_label} live (PID {pid_str}, next event seq {})\n",
                boundaries.observed.next_event_seq
            ),
        );
    }

    return Err(miette::miette!(
        "unexpected status response from recording actor"
    ));
}

#[derive(serde::Serialize)]
struct LiveStatusResponse<'session> {
    kind: &'static str,
    session: &'session str,
    lifecycle: &'static str,
    session_state: &'static str,
    pid: Option<u32>,
    boundary: serde_json::Value,
    clock_sample: ClockSample,
}

pub(in crate::cli) fn run_stop(
    response: &mut ResponseWriter,
    arguments: StopArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == RegistryLifecycle::Completed {
        let completion = wait_for_completion(&resolved.path, Duration::ZERO).into_diagnostic()?;

        let payload = serde_json::json!({
            "kind": "completed",
            "completion": completion,
        });

        if arguments.json {
            return write_json_pretty(response, &payload);
        }
        return write_stdout(
            response,
            format!("Session {} already completed\n", resolved.path.display()),
        );
    }

    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "stop", ExitCode::FAILURE);
    }

    let mut control_request = ControlRequest::new(ControlCommand::Stop);

    if let Some(request_id) = arguments.request_id {
        control_request.request_id = request_id;
    }

    control_request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let acknowledged = match send_cli_control(response, &resolved.path, &control_request, "stop")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };

    if !arguments.wait {
        if arguments.json {
            return write_json_pretty(response, &compact_control_data(&acknowledged)?);
        }
        return write_stdout(
            response,
            format!("Session {} stop requested\n", resolved.path.display()),
        );
    }

    let completion =
        wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))
            .into_diagnostic()?;

    let payload = serde_json::json!({
        "kind": "completed",
        "completion": completion,
    });

    if arguments.json {
        return write_json_pretty(response, &payload);
    }
    return write_stdout(
        response,
        format!("Session {} stopped\n", resolved.path.display()),
    );
}
