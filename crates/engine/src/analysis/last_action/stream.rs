use std::{fmt::Display, path::Path, sync::atomic::AtomicBool};

use rusqlite::{params, Connection, OptionalExtension as _};
use termtrace_evidence::{
    event_log::EventLogCursor,
    request_journal::{MutationJournalCursor, MUTATION_JOURNAL_NAME},
    session::validate_completed_session,
};
use termtrace_model::{
    execution::event::{Event, OperationStatus},
    terminal::{
        analysis::{ActionOutputSummary, DiagnosticCheck, LastActionAnalysis, LastActionBoundary},
        observation::StatePoint,
        timeline::TimelineItem,
    },
};
use uuid::Uuid;

use super::{bounded_action_findings, finish_analysis, task_error, ActionAnalysisInputs};
use crate::{
    analysis::diagnostics::diagnose_session,
    diff::diff_inspections_with_semantic_states,
    replay::{trust::SourceIdentity, visit_session_states_while, StateInspection},
    timeline::stream::TokenCursor,
    Result,
};

/// Analyze the latest input request without retaining canonical event or state history.
///
/// # Errors
/// Rejects invalid thresholds, incomplete or changing evidence, missing input results,
/// and failures to replay or create the temporary disk index.
pub fn analyze_last_action_session(
    session: &Path,
    broad_change_percent: u8,
) -> Result<LastActionAnalysis> {
    if !(1..=100).contains(&broad_change_percent) {
        return Err(task_error(
            "broad-change percentage must be between 1 and 100",
        ));
    }

    let source = SourceIdentity::capture(session)?;
    let completion = validate_completed_session(session)?;
    let action = last_input_action(session)?;
    let mut before = None;
    let mut after = None;
    let mut presentation_eligible_states = 0_u64;
    visit_session_states_while(session, &AtomicBool::new(false), |record, state| {
        if record
            .event_seq
            .is_none_or(|seq| return seq <= action.request_event_start_seq)
        {
            before = Some(StateInspection {
                record: record.clone(),
                state: state.clone(),
            });
        } else if record.presented {
            presentation_eligible_states = presentation_eligible_states.saturating_add(1);
        }

        after = Some(StateInspection {
            record: record.clone(),
            state: state.clone(),
        });

        return Ok(true);
    })?;

    let before =
        before.ok_or_else(|| return task_error("no state matches action start sequence"))?;
    let after = after.ok_or_else(|| return task_error("no state matches final sequence"))?;
    let (difference, before, after) = diff_inspections_with_semantic_states(
        StatePoint::EventSeq {
            event_seq: action.request_event_start_seq,
        },
        StatePoint::StateId {
            state_id: after.record.state_id,
        },
        before,
        after,
    );

    let mut output_generated = ActionOutputSummary {
        event_count: 0,
        byte_count: 0,
    };

    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    while let Some(record) = cursor.next_record()? {
        if record.seq > action.intent_event_seq
            && let Event::Output { data } = record.event
        {
            output_generated.event_count = output_generated.event_count.saturating_add(1);
            output_generated.byte_count = output_generated
                .byte_count
                .saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        }
    }

    let mut tokens = TokenCursor::new(session, None)?;
    let mut first_action_token_id = None;

    while let Some(entry) = tokens.next()? {
        if let TimelineItem::VtToken { token } = entry.item
            && token
                .source
                .iter()
                .any(|span| return span.event_seq > action.intent_event_seq)
        {
            first_action_token_id = Some(token.token_id);
            break;
        }
    }

    let report = diagnose_session(session, DiagnosticCheck::all(), broad_change_percent, &[])?;
    let (findings, findings_truncated) = bounded_action_findings(
        report.findings,
        difference.from.state_id,
        difference.to.state_id,
        first_action_token_id,
    );

    source.check(session)?;

    return Ok(finish_analysis(ActionAnalysisInputs {
        session_id: completion.session_id,
        source_final_event_hash: completion.final_event_hash,
        action,
        difference,
        before,
        after,
        output_generated,
        presentation_eligible_states,
        broad_change_percent,
        findings,
        findings_truncated,
    }));
}

fn index_error(error: impl Display) -> crate::Error {
    return task_error(format!("last-action temporary index: {error}"));
}

// Big-endian keys preserve the full unsigned sequence range and sort in canonical order.
fn last_input_action(session: &Path) -> Result<LastActionBoundary> {
    let directory = tempfile::tempdir().map_err(index_error)?;
    let connection =
        Connection::open(directory.path().join("actions.sqlite3")).map_err(index_error)?;
    connection
        .execute_batch(
            "PRAGMA cache_size=-8192; PRAGMA temp_store=FILE;
        CREATE TABLE intents (seq BLOB PRIMARY KEY, operation TEXT NOT NULL, bytes BLOB NOT NULL);
        CREATE TABLE results (operation TEXT PRIMARY KEY, seq BLOB NOT NULL, result BLOB NOT NULL);
        BEGIN;",
        )
        .map_err(index_error)?;

    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    let mut last_intent_seq = None;
    {
        let mut intent_insert = connection
            .prepare("INSERT INTO intents VALUES (?1, ?2, ?3)")
            .map_err(index_error)?;
        let mut result_insert = connection
            .prepare("INSERT INTO results VALUES (?1, ?2, ?3)")
            .map_err(index_error)?;

        while let Some(record) = cursor.next_record()? {
            match record.event {
                Event::InputIntent {
                    operation_id, data, ..
                } => {
                    let bytes = u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX);
                    intent_insert
                        .execute(params![
                            record.seq.to_be_bytes(),
                            operation_id.to_string(),
                            bytes.to_be_bytes()
                        ])
                        .map_err(index_error)?;

                    last_intent_seq = Some(record.seq);
                }
                Event::InputResult {
                    operation_id,
                    status,
                    bytes_written,
                    error_code,
                    ..
                } => {
                    let bytes = serde_json::to_vec(&(status, bytes_written, error_code))
                        .map_err(index_error)?;
                    result_insert
                        .execute(params![
                            operation_id.to_string(),
                            record.seq.to_be_bytes(),
                            bytes
                        ])
                        .map_err(index_error)?;
                }
                _ => {}
            }
        }
    }

    connection.execute_batch("COMMIT").map_err(index_error)?;

    let last_intent_seq = last_intent_seq
        .ok_or_else(|| return task_error("session contains no canonical input action"))?;

    let mut journal = MutationJournalCursor::open(&session.join(MUTATION_JOURNAL_NAME))?;
    let mut selected = None;
    let mut contains_input = connection
        .prepare("SELECT 1 FROM intents WHERE seq >= ?1 AND seq < ?2 LIMIT 1")
        .map_err(index_error)?;

    while let Some(request) = journal.next_record()? {
        let present = contains_input
            .query_row(
                params![
                    request.event_start_seq.to_be_bytes(),
                    request.event_end_seq.to_be_bytes()
                ],
                |_| return Ok(()),
            )
            .optional()
            .map_err(index_error)?
            .is_some();

        if present {
            selected = Some(request);
        }
    }

    let (request_id, start, end) = selected.map_or_else(
        || return (None, last_intent_seq, last_intent_seq.saturating_add(2)),
        |request| {
            return (
                Some(request.request_id),
                request.event_start_seq,
                request.event_end_seq,
            );
        },
    );

    let mut statement = connection
        .prepare(
            "SELECT i.seq, i.operation, i.bytes, r.seq, r.result
        FROM intents i LEFT JOIN results r ON r.operation=i.operation
        WHERE i.seq >= ?1 AND i.seq < ?2 ORDER BY i.seq",
        )
        .map_err(index_error)?;
    let mut rows = statement
        .query(params![start.to_be_bytes(), end.to_be_bytes()])
        .map_err(index_error)?;

    let mut action = LastActionBoundary {
        request_id,
        operation_ids: Vec::new(),
        atom_count: 0,
        request_event_start_seq: start,
        request_event_end_seq: end,
        intent_event_seq: 0,
        result_event_seq: 0,
        intended_bytes: 0,
        status: OperationStatus::Applied,
        bytes_written: 0,
        error_code: None,
    };
    while let Some(row) = rows.next().map_err(index_error)? {
        let seq = u64::from_be_bytes(row.get::<_, [u8; 8]>(0).map_err(index_error)?);
        let operation: String = row.get(1).map_err(index_error)?;
        let bytes = u64::from_be_bytes(row.get::<_, [u8; 8]>(2).map_err(index_error)?);
        let result_seq = row
            .get::<_, Option<[u8; 8]>>(3)
            .map_err(index_error)?
            .map(u64::from_be_bytes)
            .filter(|result| return *result > seq)
            .ok_or_else(|| return task_error("input intent has no canonical input result"))?;

        let result: Vec<u8> = row.get(4).map_err(index_error)?;
        let (status, written, error): (OperationStatus, u64, Option<String>) =
            serde_json::from_slice(&result).map_err(index_error)?;

        if action.operation_ids.is_empty() {
            action.intent_event_seq = seq;
        }

        action
            .operation_ids
            .push(Uuid::parse_str(&operation).map_err(index_error)?);

        action.intended_bytes = action.intended_bytes.saturating_add(bytes);
        action.bytes_written = action.bytes_written.saturating_add(written);
        action.result_event_seq = result_seq;

        if status == OperationStatus::Failed {
            action.status = status;

            if action.error_code.is_none() {
                action.error_code = error;
            }
        }
    }

    action.atom_count = u32::try_from(action.operation_ids.len()).unwrap_or(u32::MAX);

    return Ok(action);
}
