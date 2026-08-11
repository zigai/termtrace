use super::*;

pub(super) fn run_screen(arguments: ScreenArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if !matches!(
        resolved.lifecycle,
        RegistryLifecycle::Live | RegistryLifecycle::Completed
    ) {
        return write_session_lifecycle(&resolved, "screen", ExitCode::FAILURE);
    }
    let lifecycle = resolved.lifecycle;
    let session = resolved.path;
    let observation = observe_session_with_options(&session, StatePoint::LatestPresented, false)?;
    let mut output = serde_json::json!({
        "v": 1,
        "ok": true,
        "type": "screen",
        "session": session,
        "lifecycle": lifecycle,
        "state": observation.state_hash,
        "quality": observation.capture_quality,
        "size": [observation.frame.cols, observation.frame.rows],
        "ended": observation.source_completed,
    });
    if arguments.since.as_deref() == Some(observation.state_hash.as_str()) {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("screen response is not an object"))?
            .insert("unchanged".to_owned(), serde_json::Value::Bool(true));
        return write_json_line(&output);
    }
    let screen = match arguments.screen {
        ScreenEncoding::Exact => serde_json::Value::String(observation.frame.viewport_text),
        ScreenEncoding::Text => serde_json::Value::String(
            observation
                .frame
                .viewport_text
                .trim_end_matches(|character: char| {
                    return character == ' ' || character == '\n' || character == '\r';
                })
                .to_owned(),
        ),
        ScreenEncoding::Sparse => sparse_screen(&observation.frame.viewport_text),
        ScreenEncoding::Diff => {
            let since = arguments
                .since
                .ok_or_else(|| miette::miette!("--screen diff requires --since STATE_HASH"))?;
            let point = StatePoint::StateHash {
                state_hash: since.clone(),
            };
            let prior = if session.join("completion.json").is_file() {
                observe_session_with_options(&session, point, false)?
            } else {
                let _ = request(&session, ControlCommand::Status)?;
                observe_validated_prefix(&session, point, false)?
            };
            diff_screen(
                &prior.frame.viewport_text,
                &observation.frame.viewport_text,
                since,
            )
        }
        ScreenEncoding::None => serde_json::Value::Null,
    };
    if arguments.screen != ScreenEncoding::None {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("screen response is not an object"))?
            .insert("screen".to_owned(), screen);
    }
    return write_json_line(&output);
}

fn sparse_screen(viewport_text: &str) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = viewport_text
        .lines()
        .enumerate()
        .filter_map(|(row, text)| {
            let text = text.trim_end();
            return (!text.is_empty()).then(|| serde_json::json!([row, text]));
        })
        .collect();
    let total_rows = viewport_text.lines().count();
    return serde_json::json!({
        "encoding": "rows",
        "rows": rows,
        "blank_rows_omitted": total_rows.saturating_sub(rows.len()),
    });
}

fn diff_screen(before: &str, after: &str, since: String) -> serde_json::Value {
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    let row_count = before_rows.len().max(after_rows.len());
    let rows: Vec<serde_json::Value> = (0..row_count)
        .filter_map(|row| {
            let old = before_rows.get(row).copied().unwrap_or_default();
            let new = after_rows.get(row).copied().unwrap_or_default();
            return (old != new).then(|| serde_json::json!([row, new.trim_end()]));
        })
        .collect();
    return serde_json::json!({
        "encoding": "diff_rows",
        "since": since,
        "rows": rows,
    });
}

pub(super) fn run_act(arguments: ActArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "act", ExitCode::FAILURE);
    }
    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let mut atoms: Vec<InputSequenceAtom> = Vec::new();
    for atom in &arguments.atoms {
        if arguments.pace.is_zero() || !atom.starts_with("text:") {
            let operation_id = sequence_operation_id(request_id, atoms.len());
            atoms.push(input_sequence_atom_for_cli(
                atom,
                operation_id,
                arguments.intent.clone(),
            )?);
            continue;
        }
        let text = atom.strip_prefix("text:").unwrap_or_default();
        for scalar in text.chars() {
            atoms.push(InputSequenceAtom::Bytes {
                operation_id: sequence_operation_id(request_id, atoms.len()),
                data: ByteString::new(scalar.to_string().into_bytes()),
                input_kind: InputKind::Text,
                caller_intent: arguments.intent.clone(),
            });
        }
    }
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let postcondition = action_postcondition(&arguments);
    let mut request = ControlRequest::new(ControlCommand::Act {
        atoms,
        pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
        include_atom_outcomes: arguments.verbose,
        postcondition,
    });
    request.request_id = request_id;
    request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let response = match send_request_response(&resolved.path, &request) {
        Ok(response) => response,
        Err(error) => {
            write_json_line(&serde_json::json!({
                "v": 1,
                "ok": false,
                "type": "action",
                "code": "control_transport_failed",
                "message": error.to_string(),
                "request_id": request.request_id,
                "input": {
                    "applied": "unknown",
                    "atoms": serde_json::Value::Null,
                    "bytes": serde_json::Value::Null,
                    "retry": "same_request_id_only",
                },
                "postcondition": {
                    "kind": "not_started",
                    "matched": false,
                    "outcome": "not_started",
                },
                "observation": serde_json::Value::Null,
                "ended": resolved.path.join("completion.json").is_file(),
                "boundaries": serde_json::Value::Null,
            }))?;
            return Ok(ExitCode::FAILURE);
        }
    };
    match response.result {
        ControlResult::Ok { data } => {
            let (output, matched) = action_applied_response(response.request_id, &data)?;
            write_json_line(&output)?;
            return Ok(if matched {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            let output = action_error_response(
                response.request_id,
                code,
                message,
                details.as_ref(),
                resolved.path.join("completion.json").is_file(),
            );
            write_json_line(&output)?;
            return Ok(ExitCode::FAILURE);
        }
    }
}

fn action_postcondition(arguments: &ActArgs) -> ActionPostcondition {
    if let Some(stable_for) = arguments.until_idle {
        return ActionPostcondition::Idle {
            stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }
    if let Some(text) = &arguments.until_screen {
        return ActionPostcondition::Screen {
            predicate: ScreenPredicate::Contains { text: text.clone() },
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }
    if let Some(pattern) = &arguments.until_screen_regex {
        return ActionPostcondition::Screen {
            predicate: ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }
    return ActionPostcondition::None;
}

fn action_applied_response(
    request_id: uuid::Uuid,
    data: &ControlData,
) -> Result<(serde_json::Value, bool)> {
    let ControlData::ActionApplied {
        input,
        postcondition,
        boundaries,
    } = data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected action payload"
        ));
    };
    if input.sequence_id != request_id {
        return Err(miette::miette!(
            "recording actor returned an action input identifier that does not match the request"
        ));
    }
    let mut input_summary = serde_json::json!({
        "applied": "all",
        "atoms": input.atom_count,
        "bytes": input.bytes_written,
        "pace_ns": input.pace_ns,
    });
    if !input.atoms.is_empty() {
        input_summary
            .as_object_mut()
            .ok_or_else(|| miette::miette!("action input response is not an object"))?
            .insert(
                "outcomes".to_owned(),
                serde_json::to_value(&input.atoms).into_diagnostic()?,
            );
    }
    let (postcondition, observation, ended, matched) = match postcondition {
        ActionPostconditionResult::None { matched } => (
            serde_json::json!({
                "kind": "none",
                "matched": matched,
                "outcome": "matched",
            }),
            serde_json::Value::Null,
            false,
            *matched,
        ),
        ActionPostconditionResult::Idle {
            matched,
            viewport_text,
            state_hash,
            quality,
            cols,
            rows,
        } => (
            serde_json::json!({
                "kind": "idle",
                "matched": matched,
                "outcome": quality,
            }),
            serde_json::json!({
                "state": state_hash,
                "quality": quality,
                "size": [cols, rows],
                "screen": viewport_text,
            }),
            matches!(
                quality,
                StableCaptureQuality::ChildCompleted | StableCaptureQuality::OutputClosed
            ),
            *matched,
        ),
        ActionPostconditionResult::Screen {
            matched,
            viewport_text,
            state_hash,
            completion_quality,
            cols,
            rows,
            child_completed,
        } => (
            serde_json::json!({
                "kind": "screen",
                "matched": matched,
                "outcome": completion_quality,
            }),
            serde_json::json!({
                "state": state_hash,
                "quality": completion_quality,
                "size": [cols, rows],
                "screen": viewport_text,
            }),
            *child_completed || completion_quality == "output_closed",
            *matched,
        ),
    };
    let mut output = serde_json::json!({
        "v": 1,
        "ok": matched,
        "type": "action",
        "request_id": request_id,
        "input": input_summary,
        "postcondition": postcondition,
        "observation": observation,
        "ended": ended,
        "boundaries": boundaries,
    });
    if !matched {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("action response is not an object"))?
            .insert(
                "code".to_owned(),
                serde_json::Value::String("postcondition_failed".to_owned()),
            );
    }
    return Ok((output, matched));
}

fn action_error_response(
    request_id: uuid::Uuid,
    code: String,
    message: String,
    details: Option<&ControlErrorDetails>,
    ended: bool,
) -> serde_json::Value {
    let sequence = details.and_then(|details| return details.input_sequence.as_ref());
    let (applied, atoms, bytes, prefix) = match sequence {
        Some(sequence)
            if sequence.atoms_applied != 0 || sequence.failed_atom_bytes_written != 0 =>
        {
            (
                "partial",
                serde_json::json!(sequence.atoms_applied),
                serde_json::json!(sequence.bytes_written),
                serde_json::json!({
                    "atoms": sequence.atoms_applied,
                    "bytes": sequence.bytes_written,
                    "failed_atom": sequence.failed_atom_index,
                    "failed_operation_id": sequence.failed_operation_id,
                    "failed_atom_bytes": sequence.failed_atom_bytes_written,
                    "first_intent_event_seq": sequence.first_intent_event_seq,
                    "last_result_event_seq": sequence.last_result_event_seq,
                    "resume_from_atom": sequence.resume_from_atom_index,
                }),
            )
        }
        Some(_) => (
            "none",
            serde_json::json!(0),
            serde_json::json!(0),
            serde_json::Value::Null,
        ),
        None => (
            "none",
            serde_json::json!(0),
            serde_json::json!(0),
            serde_json::Value::Null,
        ),
    };
    let input = if prefix.is_null() {
        serde_json::json!({
            "applied": applied,
            "atoms": atoms,
            "bytes": bytes,
            "retry": input_retry(applied),
        })
    } else {
        serde_json::json!({
            "applied": applied,
            "atoms": atoms,
            "bytes": bytes,
            "prefix": prefix,
            "retry": input_retry(applied),
        })
    };
    let observation = details.map_or(serde_json::Value::Null, |details| {
        return serde_json::json!({
            "state": details.current_presented_state_hash,
            "scope": details.scope,
        });
    });
    let boundaries = details.map_or(serde_json::Value::Null, |details| {
        return serde_json::json!(details.boundaries);
    });
    return serde_json::json!({
        "v": 1,
        "ok": false,
        "type": "action",
        "code": code,
        "message": message,
        "request_id": request_id,
        "input": input,
        "postcondition": {
            "kind": "not_started",
            "matched": false,
            "outcome": "not_started",
        },
        "observation": observation,
        "ended": ended,
        "boundaries": boundaries,
    });
}

fn input_retry(applied: &str) -> &'static str {
    return match applied {
        "partial" => "inspect_before_retry",
        "unknown" => "same_request_id_only",
        _ => "safe_new_request",
    };
}

fn input_failure_application(details: Option<&ControlErrorDetails>) -> &'static str {
    let Some(sequence) = details.and_then(|details| return details.input_sequence.as_ref()) else {
        return "none";
    };
    if sequence.atoms_applied == 0 && sequence.failed_atom_bytes_written == 0 {
        return "none";
    }
    return "partial";
}

pub(super) fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let timeout = arguments
        .timeout
        .unwrap_or_else(|| return Duration::from_millis(arguments.timeout_ms));
    if let Some(stable_for) = arguments.idle {
        return run_wait_idle(WaitIdleArgs {
            session: arguments.session,
            stable_for,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            allow_incomplete: false,
            require_live_stability: false,
            full_state: false,
        });
    }
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "wait", ExitCode::FAILURE);
    }
    let session = resolved.path;
    if let Some(output) = arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }
        return write_json_line(&wait_for_output(session, output.as_bytes(), timeout)?);
    }
    let screen = arguments.screen.or(arguments.screen_compat);
    let predicate = match (
        screen,
        arguments.screen_absent,
        arguments.screen_regex,
        arguments.row_contains,
        arguments.region_contains,
        arguments.cursor_at,
        arguments.cursor_visible,
        arguments.cursor_shape,
        arguments.mode,
        arguments.mode_disabled,
        arguments.state_changed,
        arguments.state_unchanged,
    ) {
        (Some(text), None, None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Contains { text }
        }
        (None, Some(text), None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Absent { text }
        }
        (None, None, Some(pattern), None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::MatchesRegex { pattern }
        }
        (None, None, None, Some(value), None, None, None, None, None, None, None, None) => {
            let (row, text) = parse_row_predicate(&value)?;
            ScreenPredicate::RowContains { row, text }
        }
        (None, None, None, None, Some(value), None, None, None, None, None, None, None) => {
            let (region, text) = parse_region_predicate(&value)?;
            ScreenPredicate::RegionContains { region, text }
        }
        (None, None, None, None, None, Some(value), None, None, None, None, None, None) => {
            let (column, row) = parse_cursor_position(&value)?;
            ScreenPredicate::Cursor {
                column: Some(column),
                row: Some(row),
                visible: None,
                shape: None,
            }
        }
        (None, None, None, None, None, None, Some(visible), None, None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: Some(visible),
                shape: None,
            }
        }
        (None, None, None, None, None, None, None, Some(shape), None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: None,
                shape: Some(shape.into()),
            }
        }
        (None, None, None, None, None, None, None, None, Some(mode), None, None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: true,
            }
        }
        (None, None, None, None, None, None, None, None, None, Some(mode), None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: false,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, Some(hash), None) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Changed,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, None, Some(hash)) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Unchanged,
            }
        }
        _ => return Err(miette::miette!("exactly one wait predicate is required")),
    };
    return write_json_line(&wait_for_screen(
        session,
        predicate,
        arguments.stable_for,
        timeout,
    )?);
}

pub(super) fn run_wait_idle(arguments: WaitIdleArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == RegistryLifecycle::Completed {
        if arguments.require_live_stability {
            return write_session_lifecycle(&resolved, "wait_idle", ExitCode::FAILURE);
        }
        return write_completed_stable_snapshot(&resolved, arguments.full_state);
    }
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "wait_idle", ExitCode::FAILURE);
    }
    let data = send_request(
        resolved.path,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: u64::try_from(arguments.stable_for.as_nanos()).unwrap_or(u64::MAX),
            timeout_ns: arguments.timeout_ms.saturating_mul(1_000_000),
        }),
    )?;
    let ControlData::StableSnapshot {
        semantic_state_zstd: _,
        viewport_text,
        state_hash,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        first_observed_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        modes,
        boundaries,
    } = &data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected stable-capture payload"
        ));
    };
    if !arguments.allow_incomplete && *quality != StableCaptureQuality::Stable {
        return Err(miette::miette!(
            "stable capture ended with {}; pass --allow-incomplete to receive the typed snapshot",
            quality.as_str()
        ));
    }
    if arguments.full_state {
        return write_json_line(&data);
    }
    return write_json_line(&serde_json::json!({
        "kind": "stable_snapshot",
        "quality": quality,
        "state_hash": state_hash,
        "viewport_text": viewport_text,
        "geometry": {
            "cols": cols,
            "rows": rows,
        },
        "cursor": {
            "column": cursor_column,
            "row": cursor_row,
            "visible": cursor_visible,
        },
        "first_observed_monotonic_ns": first_observed_monotonic_ns,
        "observed_monotonic_ns": observed_monotonic_ns,
        "stable_for_ns": stable_for_ns,
        "modes": modes,
        "boundary": compact_boundaries(boundaries),
    }));
}

fn compact_boundaries(boundaries: &ActorBoundaries) -> serde_json::Value {
    if boundaries.durable == boundaries.observed {
        return serde_json::json!({
            "durable": true,
            "next_event_seq": boundaries.observed.next_event_seq,
            "event_hash": boundaries.observed.event_hash,
            "output_bytes": boundaries.observed.output_bytes,
        });
    }
    return serde_json::json!({
        "durable": false,
        "durable_boundary": boundaries.durable,
        "observed_boundary": boundaries.observed,
    });
}

fn write_completed_stable_snapshot(
    resolved: &ResolvedSessionRef,
    full_state: bool,
) -> Result<ExitCode> {
    let observation =
        observe_session_with_options(&resolved.path, StatePoint::LatestPresented, false)?;
    if full_state {
        return write_json_line(&serde_json::json!({
            "kind": "stable_snapshot",
            "quality": "completed",
            "observation": observation,
        }));
    }
    return write_json_line(&serde_json::json!({
        "kind": "stable_snapshot",
        "quality": "completed",
        "state_hash": observation.state_hash,
        "viewport_text": observation.frame.viewport_text,
        "geometry": {
            "cols": observation.frame.cols,
            "rows": observation.frame.rows,
        },
        "cursor": {
            "column": observation.frame.cursor_column,
            "row": observation.frame.cursor_row,
            "visible": observation.frame.cursor_visible,
        },
        "modes": observation.frame.modes,
        "boundary": observation.boundary,
    }));
}

fn compact_control_data(data: &ControlData) -> Result<serde_json::Value> {
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

enum CliControlOutcome {
    Applied(Box<ControlData>),
    Rejected(ExitCode),
}

fn send_cli_control(
    session: &Path,
    request: &ControlRequest,
    operation: &'static str,
) -> Result<CliControlOutcome> {
    let response = send_request_response(session, request)?;
    match response.result {
        ControlResult::Ok { data } => return Ok(CliControlOutcome::Applied(Box::new(data))),
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            write_json_line(&serde_json::json!({
                "kind": "control_rejected",
                "operation": operation,
                "code": code,
                "message": message,
                "details": details,
                "request_id": response.request_id,
                "applied": "none",
                "retry": "safe_new_request",
                "session_state": "live",
            }))?;
            return Ok(CliControlOutcome::Rejected(ExitCode::FAILURE));
        }
    }
}

pub(super) fn run_control(session: PathBuf, command: ControlCommand) -> Result<ExitCode> {
    return run_control_guarded(session, command, None, None);
}

pub(super) fn run_control_guarded(
    session: PathBuf,
    command: ControlCommand,
    expected_presented_state_hash: Option<String>,
    request_id: Option<uuid::Uuid>,
) -> Result<ExitCode> {
    let resolved = resolve_session(session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "control", ExitCode::FAILURE);
    }
    let mut control_request = ControlRequest::new(command);
    if let Some(request_id) = request_id {
        control_request.request_id = request_id;
    }
    control_request.expected_presented_state_hash = expected_presented_state_hash;
    let data = match send_cli_control(&resolved.path, &control_request, "control")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };
    return write_json_line(&compact_control_data(&data)?);
}

pub(super) fn run_status(arguments: StatusArgs) -> Result<ExitCode> {
    let requested_session = arguments.session;
    let session_label = requested_session.display().to_string();
    let resolved = resolve_session(requested_session)?;
    if arguments.wait_complete || resolved.lifecycle == RegistryLifecycle::Completed {
        let completion =
            wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "session": session_label,
            "lifecycle": "completed",
            "completion": completion,
        }));
    }
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "status", ExitCode::SUCCESS);
    }
    let mut status = compact_control_data(&request(resolved.path, ControlCommand::Status)?)?;
    let status = status
        .as_object_mut()
        .ok_or_else(|| miette::miette!("live status response is not an object"))?;
    status.insert("session".to_owned(), serde_json::json!(session_label));
    status.insert("lifecycle".to_owned(), serde_json::json!("live"));
    return write_json_line(status);
}

pub(super) fn run_stop(arguments: StopArgs) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == RegistryLifecycle::Completed {
        let completion = wait_for_completion(&resolved.path, Duration::ZERO)?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "stop", ExitCode::FAILURE);
    }
    let mut control_request = ControlRequest::new(ControlCommand::Stop);
    if let Some(request_id) = arguments.request_id {
        control_request.request_id = request_id;
    }
    control_request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let acknowledged = match send_cli_control(&resolved.path, &control_request, "stop")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };
    if !arguments.wait {
        return write_json_line(&compact_control_data(&acknowledged)?);
    }
    let completion =
        wait_for_completion(&resolved.path, Duration::from_millis(arguments.timeout_ms))?;
    return write_json_line(&serde_json::json!({
        "kind": "completed",
        "completion": completion,
    }));
}

pub(super) fn run_send(arguments: SendArgs) -> Result<ExitCode> {
    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let mut atoms: Vec<InputSequenceAtom> = Vec::new();
    if arguments.stdin {
        let mut data: Vec<u8> = Vec::new();
        io::stdin().read_to_end(&mut data).into_diagnostic()?;
        atoms.push(InputSequenceAtom::Bytes {
            operation_id: sequence_operation_id(request_id, 0),
            data: ByteString::new(data),
            input_kind: InputKind::Raw,
            caller_intent: arguments.intent.clone(),
        });
    } else {
        for atom in &arguments.atoms {
            if arguments.pace.is_zero() || !atom.starts_with("text:") {
                let operation_id = sequence_operation_id(request_id, atoms.len());
                atoms.push(input_sequence_atom_for_cli(
                    atom,
                    operation_id,
                    arguments.intent.clone(),
                )?);
                continue;
            }
            let text = atom.strip_prefix("text:").unwrap_or_default();
            for scalar in text.chars() {
                atoms.push(InputSequenceAtom::Bytes {
                    operation_id: sequence_operation_id(request_id, atoms.len()),
                    data: ByteString::new(scalar.to_string().into_bytes()),
                    input_kind: InputKind::Text,
                    caller_intent: arguments.intent.clone(),
                });
            }
        }
    }
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "send", ExitCode::FAILURE);
    }
    let mut request = ControlRequest::new(ControlCommand::InputSequence {
        atoms,
        pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
        include_atom_outcomes: arguments.verbose,
    });
    request.request_id = request_id;
    request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let response = match send_request_response(&resolved.path, &request) {
        Ok(response) => response,
        Err(error) => {
            write_json_line(&serde_json::json!({
                "kind": "input_sequence_failed",
                "sequence_id": request.request_id,
                "code": "transport_lost",
                "message": error.to_string(),
                "applied": "unknown",
                "retry": "same_request_id_only",
                "session_state": "unknown",
            }))?;
            return Ok(ExitCode::FAILURE);
        }
    };
    let data = match response.result {
        ControlResult::Ok { data } => data,
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            let applied = input_failure_application(details.as_ref());
            write_json_line(&serde_json::json!({
                "kind": "input_sequence_failed",
                "sequence_id": request.request_id,
                "code": code,
                "message": message,
                "details": details,
                "applied": applied,
                "retry": input_retry(applied),
                "session_state": "live",
            }))?;
            return Ok(ExitCode::FAILURE);
        }
    };
    let ControlData::InputSequenceApplied {
        sequence_id,
        atom_count,
        bytes_written,
        first_intent_event_seq,
        last_result_event_seq,
        pace_ns,
        atoms,
        boundaries,
    } = data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected input-sequence payload"
        ));
    };
    let mut output = serde_json::json!({
        "kind": "input_sequence_applied",
        "sequence_id": sequence_id,
        "atom_count": atom_count,
        "bytes_written": bytes_written,
        "first_intent_event_seq": first_intent_event_seq,
        "last_result_event_seq": last_result_event_seq,
        "pace_ns": pace_ns,
        "boundary": compact_boundaries(&boundaries),
    });
    if arguments.verbose {
        output
            .as_object_mut()
            .ok_or_else(|| return miette::miette!("input-sequence output is not an object"))?
            .insert(
                "atoms".to_owned(),
                serde_json::to_value(atoms).into_diagnostic()?,
            );
    }
    return write_json_line(&output);
}

fn input_sequence_atom_for_cli(
    atom: &str,
    operation_id: uuid::Uuid,
    caller_intent: Option<String>,
) -> Result<InputSequenceAtom> {
    if let Some((key, modifiers)) = parse_named_key(atom)? {
        return Ok(InputSequenceAtom::Key {
            operation_id,
            key,
            modifiers,
            caller_intent,
        });
    }
    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(paste.as_bytes().to_vec()),
            caller_intent,
        });
    }
    let (data, input_kind) = if let Some(text) = atom.strip_prefix("text:") {
        (text.as_bytes().to_vec(), InputKind::Text)
    } else if let Some(raw) = atom.strip_prefix("raw:") {
        (raw.as_bytes().to_vec(), InputKind::Raw)
    } else if let Some(path) = atom.strip_prefix("raw-file:") {
        (fs_err::read(path).into_diagnostic()?, InputKind::Raw)
    } else if atom.chars().count() == 1
        && atom.chars().all(|character| return !character.is_control())
    {
        (atom.as_bytes().to_vec(), InputKind::Text)
    } else {
        return Err(miette::miette!(
            "unknown input atom; run `termtrace control keys` or use text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH"
        ));
    };
    return Ok(InputSequenceAtom::Bytes {
        operation_id,
        data: ByteString::new(data),
        input_kind,
        caller_intent,
    });
}

fn sequence_operation_id(sequence_id: uuid::Uuid, index: usize) -> uuid::Uuid {
    let atom_number = u128::try_from(index).unwrap_or(u128::MAX).saturating_add(1);
    return uuid::Uuid::from_u128(sequence_id.as_u128() ^ atom_number);
}

fn parse_named_key(atom: &str) -> Result<Option<(NamedKey, KeyModifiers)>> {
    let normalized = atom.to_ascii_lowercase().replace('+', "-");
    let mut remaining = normalized.as_str();
    let mut modifiers = KeyModifiers::default();
    let mut modifier_count = 0_u8;
    loop {
        let (next, selected) = if let Some(next) = remaining.strip_prefix("ctrl-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }
            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("control-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }
            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("shift-") {
            if modifiers.shift {
                return Err(miette::miette!(
                    "shift modifier was supplied more than once"
                ));
            }
            modifiers.shift = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("alt-") {
            if modifiers.alt {
                return Err(miette::miette!("alt modifier was supplied more than once"));
            }
            modifiers.alt = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("super-") {
            if modifiers.super_key {
                return Err(miette::miette!(
                    "super modifier was supplied more than once"
                ));
            }
            modifiers.super_key = true;
            (next, true)
        } else {
            (remaining, false)
        };
        if !selected {
            break;
        }
        modifier_count = modifier_count.saturating_add(1);
        remaining = next;
    }
    let key = match remaining {
        "enter" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "backspace" => NamedKey::Backspace,
        "delete" => NamedKey::Delete,
        "insert" => NamedKey::Insert,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "page-up" => NamedKey::PageUp,
        "page-down" => NamedKey::PageDown,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        value if value.len() >= 2 && value.starts_with('f') => {
            let suffix = value
                .strip_prefix('f')
                .ok_or_else(|| miette::miette!("function key must be f1 through f24"))?;
            let number = suffix
                .parse::<u8>()
                .map_err(|_| miette::miette!("function key must be f1 through f24"))?;
            if !(1..=24).contains(&number) {
                return Err(miette::miette!("function key must be f1 through f24"));
            }
            NamedKey::Function(number)
        }
        value if modifier_count > 0 && value.len() == 1 => {
            let letter = value
                .chars()
                .next()
                .ok_or_else(|| miette::miette!("modified letter is missing"))?;
            if !letter.is_ascii_alphabetic() {
                return Err(miette::miette!(
                    "modified printable keys currently support ASCII letters; run `termtrace control keys`"
                ));
            }
            NamedKey::Letter(letter)
        }
        _ => return Ok(None),
    };
    return Ok(Some((key, modifiers)));
}

pub(super) fn run_keys() -> Result<ExitCode> {
    return write_json_line(&serde_json::json!({
        "format": "termtrace-key-contract",
        "version": 1,
        "named_keys": [
            "enter", "escape", "esc", "tab", "backspace", "delete", "insert",
            "home", "end", "page-up", "page-down", "up", "down", "left", "right",
            "f1..f24"
        ],
        "modifiers": ["ctrl", "shift", "alt", "super"],
        "modifier_separators": ["-", "+"],
        "modified_examples": ["ctrl-f", "ctrl+f", "ctrl-down", "shift-tab", "alt-left"],
        "text": {
            "single_printable_character": "q",
            "explicit": "text:any UTF-8 text"
        },
        "paste": "paste:any UTF-8 text",
        "raw": ["raw:exact bytes from this argument", "raw-file:PATH", "send --stdin SESSION"]
    }));
}

pub(super) fn run_resize(arguments: ResizeArgs) -> Result<ExitCode> {
    let pixel_width = arguments
        .pixel_width
        .or_else(|| return arguments.cols.checked_mul(12))
        .ok_or_else(|| miette::miette!("pixel width overflow; pass --pixel-width explicitly"))?;
    let pixel_height = arguments
        .pixel_height
        .or_else(|| return arguments.rows.checked_mul(24))
        .ok_or_else(|| miette::miette!("pixel height overflow; pass --pixel-height explicitly"))?;
    return run_control_guarded(
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
        arguments.expected_presented_state_hash,
        arguments.request_id,
    );
}

pub(super) fn run_pointer(arguments: PointerArgs) -> Result<ExitCode> {
    let (column, row, operation, strict, expected_presented_state_hash, request_id, postcondition) =
        match arguments.operation {
            PointerCommand::Click(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::Click {
                    button: click.button.into(),
                },
                click.strict,
                click.expected_presented_state_hash,
                click.request_id,
                click.postcondition,
            ),
            PointerCommand::MultiClick(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::MultiClick {
                    button: click.button.into(),
                    count: click.count,
                },
                click.strict,
                click.expected_presented_state_hash,
                click.request_id,
                click.postcondition,
            ),
            PointerCommand::Drag(drag) => (
                drag.from.0,
                drag.from.1,
                PointerOperation::Drag {
                    button: drag.button.into(),
                    to_column: drag.to.0,
                    to_row: drag.to.1,
                    steps: drag.steps,
                },
                drag.strict,
                drag.expected_presented_state_hash,
                drag.request_id,
                drag.postcondition,
            ),
            PointerCommand::Scroll(scroll) => (
                scroll.cell.0,
                scroll.cell.1,
                PointerOperation::Scroll {
                    lines: scroll.lines,
                },
                scroll.strict,
                scroll.expected_presented_state_hash,
                scroll.request_id,
                scroll.postcondition,
            ),
        };
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(&resolved, "pointer", ExitCode::FAILURE);
    }
    let mut request = ControlRequest::new(ControlCommand::Pointer {
        column,
        row,
        operation,
        strict,
    });
    request.expected_presented_state_hash = expected_presented_state_hash;
    if let Some(request_id) = request_id {
        request.request_id = request_id;
    }
    let input = match send_cli_control(&resolved.path, &request, "pointer")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };
    let predicate = match (postcondition.wait_screen, postcondition.wait_screen_absent) {
        (Some(text), None) => Some(ScreenPredicate::Contains { text }),
        (None, Some(text)) => Some(ScreenPredicate::Absent { text }),
        (None, None) => None,
        (Some(_), Some(_)) => {
            return Err(miette::miette!(
                "pointer postcondition requires exactly one screen predicate"
            ));
        }
    };
    let Some(predicate) = predicate else {
        return write_json_line(&input);
    };
    let matched = wait_for_screen(
        &resolved.path,
        predicate,
        postcondition.stable_for,
        Duration::from_millis(postcondition.timeout_ms),
    )?;
    return write_json_line(&serde_json::json!({
        "kind": "pointer_with_postcondition",
        "input": input,
        "postcondition": matched,
    }));
}

#[cfg(test)]
mod action_response_tests {
    use super::*;

    #[test]
    fn action_success_retains_input_observation_request_and_boundaries() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000001").expect("request UUID");
        let data: ControlData = serde_json::from_value(serde_json::json!({
            "kind": "action_applied",
            "input": {
                "sequence_id": request_id,
                "atom_count": 2,
                "bytes_written": 4,
                "first_intent_event_seq": 1,
                "last_result_event_seq": 4,
                "pace_ns": 1_000_000,
                "atoms": [{
                    "operation_id": "018f47d2-7ad0-7000-8000-000000000002",
                    "input_kind": "text",
                    "bytes_written": 2,
                    "intent_event_seq": 1,
                    "result_event_seq": 2
                }]
            },
            "postcondition": {
                "kind": "screen",
                "matched": false,
                "viewport_text": "current screen",
                "state_hash": "a".repeat(64),
                "completion_quality": "deadline",
                "cols": 80,
                "rows": 24,
                "child_completed": false
            },
            "boundaries": {
                "durable": {
                    "next_event_seq": 4,
                    "event_hash": "b".repeat(64),
                    "output_bytes": 10
                },
                "observed": {
                    "next_event_seq": 5,
                    "event_hash": "c".repeat(64),
                    "output_bytes": 12
                }
            }
        }))
        .expect("action payload");
        let (response, matched) =
            action_applied_response(request_id, &data).expect("action response");
        assert!(!matched);
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["applied"], "all");
        assert_eq!(response["input"]["atoms"], 2);
        assert_eq!(response["input"]["bytes"], 4);
        assert_eq!(
            response["input"]["outcomes"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(response["postcondition"]["outcome"], "deadline");
        assert_eq!(response["ended"], false);
        assert_eq!(response["observation"]["screen"], "current screen");
        assert_eq!(response["boundaries"]["durable"]["next_event_seq"], 4);
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 5);
    }

    #[test]
    fn action_partial_failure_retains_the_exact_attempted_prefix() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000010").expect("request UUID");
        let details: ControlErrorDetails = serde_json::from_value(serde_json::json!({
            "current_presented_state_hash": "d".repeat(64),
            "boundaries": {
                "durable": {
                    "next_event_seq": 7,
                    "event_hash": "e".repeat(64),
                    "output_bytes": 20
                },
                "observed": {
                    "next_event_seq": 8,
                    "event_hash": "f".repeat(64),
                    "output_bytes": 21
                }
            },
            "scope": "input_sequence_attempted_prefix",
            "input_sequence": {
                "sequence_id": request_id,
                "atom_count": 3,
                "atoms_applied": 1,
                "failed_atom_index": 1,
                "failed_operation_id": "018f47d2-7ad0-7000-8000-000000000011",
                "bytes_written": 5,
                "failed_atom_bytes_written": 2,
                "first_intent_event_seq": 2,
                "last_result_event_seq": 7,
                "resume_from_atom_index": 2
            }
        }))
        .expect("failure details");
        let response = action_error_response(
            request_id,
            "input_sequence_incomplete".to_owned(),
            "input stopped".to_owned(),
            Some(&details),
            false,
        );
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["applied"], "partial");
        assert_eq!(response["input"]["atoms"], 1);
        assert_eq!(response["input"]["bytes"], 5);
        assert_eq!(response["input"]["prefix"]["atoms"], 1);
        assert_eq!(response["input"]["prefix"]["bytes"], 5);
        assert_eq!(response["input"]["prefix"]["failed_atom"], 1);
        assert_eq!(response["input"]["prefix"]["failed_atom_bytes"], 2);
        assert_eq!(response["input"]["prefix"]["resume_from_atom"], 2);
        assert_eq!(response["postcondition"]["outcome"], "not_started");
        assert_eq!(response["observation"]["state"], "d".repeat(64));
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 8);
    }
}
