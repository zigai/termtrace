use super::*;
use std::{ffi::OsString, fmt::Write as _, os::unix::ffi::OsStringExt as _};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::protocol::control::ClockSample;

pub(super) fn compact_control_data(data: &ControlPayload) -> Result<serde_json::Value> {
    let value = match data {
        ControlPayload::Acknowledged { boundaries } => serde_json::json!({
            "kind": "acknowledged",
            "boundary": compact_boundaries(boundaries),
        }),
        ControlPayload::InputApplied {
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
        ControlPayload::InputSequenceApplied {
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
        ControlPayload::InputNoop {
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
        ControlPayload::Activated {
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
        ControlPayload::Status {
            pid,
            boundaries,
            clock_sample,
        } => serde_json::json!({
            "kind": "status",
            "pid": pid,
            "boundary": compact_boundaries(boundaries),
            "clock_sample": clock_sample,
        }),
        ControlPayload::Snapshot {
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
        ControlPayload::ActionApplied { .. }
        | ControlPayload::EventCursor { .. }
        | ControlPayload::ScreenFrame { .. }
        | ControlPayload::StableSnapshot { .. }
        | ControlPayload::ScreenWait { .. } => serde_json::to_value(data).into_diagnostic()?,
        ControlPayload::Marked {
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

pub(in crate::cli) fn run_status(
    response: &mut ResponseWriter,
    arguments: StatusArgs,
) -> Result<ExitCode> {
    if arguments.gc {
        let registry = SessionRegistry::open_default().into_diagnostic()?;
        let report = registry
            .garbage_collect_with_quota(
                arguments.older_than_days,
                arguments.keep_latest,
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
    if arguments.wait_complete || resolved.phase == SessionPhase::Completed {
        let completion = wait_for_completion(
            &resolved.path,
            millis_or(arguments.timeout_ms, defaults::STOP_TIMEOUT),
        )
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

    if resolved.phase != SessionPhase::Live {
        if arguments.json {
            return write_session_lifecycle(response, &resolved, "status", ExitCode::SUCCESS);
        }
        return write_stdout(
            response,
            format!("Session {session_label} {}\n", resolved.phase.as_str()),
        );
    }

    let data = request(resolved.path, ControlCommand::Status).into_diagnostic()?;
    if let ControlPayload::Status {
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
    if !matches!(resolved.phase, SessionPhase::Live | SessionPhase::Completed) {
        return write_session_lifecycle(response, &resolved, "stop", ExitCode::FAILURE);
    }

    let already_completed = resolved.phase == SessionPhase::Completed;
    let request = StopRequest {
        guard: MutationGuard {
            request_id: arguments.request_id,
            if_state: arguments.expected_presented_state_hash,
        },
        policy: StopPolicy::Request,
    };

    let wait = arguments
        .wait
        .then(|| return millis_or(arguments.timeout_ms, defaults::STOP_TIMEOUT));

    let outcome = match attach_session(&resolved.path)?.stop(request, wait) {
        Ok(outcome) => outcome,
        Err(error) => {
            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "control_rejected",
                    "operation": "stop",
                    "code": error.code(),
                    "message": error.to_string(),
                    "details": error.details(),
                    "effect": error.effect(),
                    "retry": error.retry(),
                }),
            )?;

            return Ok(ExitCode::FAILURE);
        }
    };

    let (payload, summary) = match outcome {
        StopOutcome::Requested(acknowledgement) => {
            (compact_control_data(&acknowledgement)?, "stop requested")
        }
        StopOutcome::Completed(completion) => (
            serde_json::json!({
                "kind": "completed",
                "completion": completion,
            }),
            if already_completed { "already completed" } else { "stopped" },
        ),
    };

    if arguments.json {
        return write_json_pretty(response, &payload);
    }
    return write_stdout(
        response,
        format!("Session {} {summary}\n", resolved.path.display()),
    );
}
