use super::*;

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
        ControlData::Status { pid, boundaries } => serde_json::json!({
            "kind": "status",
            "pid": pid,
            "boundary": compact_boundaries(boundaries),
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

pub(in crate::cli) fn run_control_guarded(
    response: &mut ResponseWriter,
    session: PathBuf,
    command: ControlCommand,
    expected_presented_state_hash: Option<String>,
    request_id: Option<uuid::Uuid>,
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
    return write_json_line(response, &compact_control_data(&data)?);
}

pub(in crate::cli) fn run_status(
    response: &mut ResponseWriter,
    arguments: StatusArgs,
) -> Result<ExitCode> {
    let requested_session = arguments.session;
    let session_label = requested_session.display().to_string();
    let resolved = resolve_session(requested_session)?;
    if arguments.wait_complete || resolved.lifecycle == RegistryLifecycle::Completed {
        let completion =
            wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))
                .into_diagnostic()?;
        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "completed",
                "session": session_label,
                "lifecycle": "completed",
                "completion": completion,
            }),
        );
    }
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "status", ExitCode::SUCCESS);
    }
    let mut status =
        compact_control_data(&request(resolved.path, ControlCommand::Status).into_diagnostic()?)?;
    let status = status
        .as_object_mut()
        .ok_or_else(|| miette::miette!("live status response is not an object"))?;
    status.insert("session".to_owned(), serde_json::json!(session_label));
    status.insert("lifecycle".to_owned(), serde_json::json!("live"));
    return write_json_line(response, status);
}

pub(in crate::cli) fn run_stop(
    response: &mut ResponseWriter,
    arguments: StopArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == RegistryLifecycle::Completed {
        let completion = wait_for_completion(&resolved.path, Duration::ZERO).into_diagnostic()?;
        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "completed",
                "completion": completion,
            }),
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
        return write_json_line(response, &compact_control_data(&acknowledged)?);
    }
    let completion =
        wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))
            .into_diagnostic()?;
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }),
    );
}
