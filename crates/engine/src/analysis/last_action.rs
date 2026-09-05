//! Task-oriented analysis of the most recent canonical input action.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use termtrace_evidence::{
    event_log::EventLog,
    request_journal::{MUTATION_JOURNAL_NAME, read_mutation_journal},
};
use termtrace_model::{
    execution::event::{Event, EventRecord, OperationStatus},
    execution::request_journal::MutationJournalRecord,
    terminal::analysis::{
        ActionModeChange, ActionOutputSummary, ActionWarning, DiagnosticCheck, DiagnosticFinding,
        FindingConfidence, LastActionAnalysis, LastActionBoundary, LastActionDiagnosticsReport,
        LastActionReport,
    },
    terminal::diff::{RowChange, ValueChange},
    terminal::observation::StatePoint,
    terminal::state::{ModeState, SemanticState, TerminalMode},
};
use termtrace_terminal::frame_event_records;
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::diagnostics::diagnose,
    diff::diff_inspections_with_semantic_states,
    replay::{StateInspection, visit_states},
};

const MAX_ACTION_WARNINGS: usize = 16;
const MAX_ACTION_FINDINGS: usize = 32;
const MAX_FINDING_FACTS: usize = 4;
const MAX_FINDING_REFERENCES: usize = 64;

fn task_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

/// Analyze effects, output, mode changes, and diagnostics for the latest input action.
///
/// # Errors
///
/// Returns an error when the percentage is outside `1..=100`, no completed input action
/// exists, or the session evidence cannot be replayed or inspected.
pub fn analyze_last_action(
    session: &Path,
    log: &EventLog,
    broad_change_percent: u8,
) -> Result<LastActionAnalysis> {
    if !(1..=100).contains(&broad_change_percent) {
        return Err(task_error(
            "broad-change percentage must be between 1 and 100",
        ));
    }
    let action = last_input_action(session, log)?;
    let from_point = StatePoint::EventSeq {
        event_seq: action.request_event_start_seq,
    };
    let mut before_inspection: Option<StateInspection> = None;
    let mut after_inspection: Option<StateInspection> = None;
    let mut presentation_eligible_states = 0_u64;

    visit_states(log, |record, state| {
        if record
            .event_seq
            .is_none_or(|event| return event <= action.request_event_start_seq)
        {
            before_inspection = Some(StateInspection {
                record: record.clone(),
                state: state.clone(),
            });
        } else if record.presented {
            presentation_eligible_states = presentation_eligible_states.saturating_add(1);
        }
        after_inspection = Some(StateInspection {
            record: record.clone(),
            state: state.clone(),
        });
        return Ok(());
    })?;

    let from_inspection = before_inspection
        .ok_or_else(|| return task_error("no state matches action start sequence"))?;
    let to_inspection =
        after_inspection.ok_or_else(|| return task_error("no state matches final sequence"))?;
    let to_point = StatePoint::StateId {
        state_id: to_inspection.record.state_id,
    };
    let (difference, before, after) =
        diff_inspections_with_semantic_states(from_point, to_point, from_inspection, to_inspection);
    let output_generated = output_summary(log, action.intent_event_seq);
    let diagnostic_report = diagnose(log, DiagnosticCheck::all(), broad_change_percent, &[])?;
    let action_token_ids = frame_event_records(&log.events)?
        .into_iter()
        .filter(|token| {
            return token
                .source
                .iter()
                .any(|source| return source.event_seq > action.intent_event_seq);
        })
        .map(|token| return token.token_id)
        .collect::<Vec<_>>();
    let (findings, findings_truncated) = bounded_action_findings(
        diagnostic_report.findings,
        difference.from.state_id,
        difference.to.state_id,
        &action_token_ids,
    );
    let (warnings, warning_limit_reached) = action_warnings(
        &difference.rows,
        &before,
        &after,
        &output_generated,
        presentation_eligible_states,
        broad_change_percent,
        &findings,
    );
    let warnings_truncated = warning_limit_reached || findings_truncated;
    let rows_changed = difference.rows.iter().map(|row| return row.row).collect();
    let cells_changed = difference.rows.iter().fold(0_u64, |total, row| {
        return total.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
    });
    let mode_changes = changed_modes(difference.modes.as_ref());
    let source_final_event_hash = log
        .events
        .last()
        .ok_or_else(|| return task_error("canonical input action has no event record"))?
        .hash
        .clone();
    let inspection = LastActionReport {
        format: "termtrace-last-action".to_owned(),
        version: 1,
        session_id: log.header.session_id,
        source_final_event_hash: source_final_event_hash.clone(),
        action: action.clone(),
        from_state_id: difference.from.state_id,
        to_state_id: difference.to.state_id,
        rows_changed,
        cells_changed,
        cursor_movement: difference.cursor,
        mode_changes,
        output_generated,
        presentation_eligible: difference.to.presented,
        presentation_eligible_states,
        warnings,
        warnings_truncated,
    };
    let diagnostics = LastActionDiagnosticsReport {
        format: "termtrace-last-action-diagnostics".to_owned(),
        version: 1,
        session_id: log.header.session_id,
        source_final_event_hash,
        action,
        from_state_id: difference.from.state_id,
        to_state_id: difference.to.state_id,
        findings,
        findings_truncated,
    };
    return Ok(LastActionAnalysis {
        inspection,
        diagnostics,
    });
}

fn last_input_action(session: &Path, log: &EventLog) -> Result<LastActionBoundary> {
    let journal = read_mutation_journal(&session.join(MUTATION_JOURNAL_NAME))?;
    for request in journal.iter().rev() {
        if let Some(action) = journaled_input_action(log, request)? {
            return Ok(action);
        }
    }
    return unjournaled_input_action(log);
}

fn journaled_input_action(
    log: &EventLog,
    request: &MutationJournalRecord,
) -> Result<Option<LastActionBoundary>> {
    let intents = log
        .events
        .iter()
        .filter(|record| {
            return record.seq >= request.event_start_seq
                && record.seq < request.event_end_seq
                && matches!(record.event, Event::InputIntent { .. });
        })
        .collect::<Vec<_>>();
    if intents.is_empty() {
        return Ok(None);
    }
    return Ok(Some(action_boundary(
        log,
        Some(request.request_id),
        request.event_start_seq,
        request.event_end_seq,
        &intents,
    )?));
}

fn unjournaled_input_action(log: &EventLog) -> Result<LastActionBoundary> {
    let intent = log
        .events
        .iter()
        .rev()
        .find(|record| return matches!(record.event, Event::InputIntent { .. }))
        .ok_or_else(|| return task_error("session contains no canonical input action"))?;
    return action_boundary(
        log,
        None,
        intent.seq,
        intent.seq.saturating_add(2),
        &[intent],
    );
}

fn action_boundary(
    log: &EventLog,
    request_id: Option<Uuid>,
    request_event_start_seq: u64,
    request_event_end_seq: u64,
    intents: &[&EventRecord],
) -> Result<LastActionBoundary> {
    let mut operation_ids = Vec::with_capacity(intents.len());
    let mut intended_bytes = 0_u64;
    let mut bytes_written = 0_u64;
    let mut status = OperationStatus::Applied;
    let mut error_code = None;
    let mut first_intent_event_seq = None;
    let mut last_result_event_seq = None;
    for intent in intents {
        let Event::InputIntent {
            operation_id, data, ..
        } = &intent.event
        else {
            return Err(task_error("selected action intent changed event type"));
        };
        operation_ids.push(*operation_id);
        intended_bytes =
            intended_bytes.saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        first_intent_event_seq.get_or_insert(intent.seq);
        let result = log
            .events
            .iter()
            .find(|record| {
                return record.seq > intent.seq
                    && matches!(
                        record.event,
                        Event::InputResult {
                            operation_id: result_id,
                            ..
                        } if result_id == *operation_id
                    );
            })
            .ok_or_else(|| return task_error("input intent has no canonical input result"))?;
        let Event::InputResult {
            status: atom_status,
            bytes_written: atom_bytes_written,
            error_code: atom_error_code,
            ..
        } = &result.event
        else {
            return Err(task_error("matched input result changed event type"));
        };
        bytes_written = bytes_written.saturating_add(*atom_bytes_written);
        last_result_event_seq = Some(result.seq);
        if *atom_status == OperationStatus::Failed {
            status = OperationStatus::Failed;
            if error_code.is_none() {
                error_code = atom_error_code.clone();
            }
        }
    }
    let intent_event_seq = first_intent_event_seq
        .ok_or_else(|| return task_error("input action has no canonical intent"))?;
    let result_event_seq = last_result_event_seq
        .ok_or_else(|| return task_error("input action has no canonical result"))?;
    return Ok(LastActionBoundary {
        request_id,
        atom_count: u32::try_from(operation_ids.len()).unwrap_or(u32::MAX),
        operation_ids,
        request_event_start_seq,
        request_event_end_seq,
        intent_event_seq,
        result_event_seq,
        intended_bytes,
        status,
        bytes_written,
        error_code,
    });
}

fn output_summary(log: &EventLog, intent_event_seq: u64) -> ActionOutputSummary {
    let (event_count, byte_count) = log
        .events
        .iter()
        .filter(|record| return record.seq > intent_event_seq)
        .filter_map(|record| match &record.event {
            Event::Output { data } => {
                return Some(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
            }
            _ => return None,
        })
        .fold((0_u64, 0_u64), |(events, bytes), len| {
            return (events.saturating_add(1), bytes.saturating_add(len));
        });
    return ActionOutputSummary {
        event_count,
        byte_count,
    };
}

fn changed_modes(change: Option<&ValueChange<ModeState>>) -> Vec<ActionModeChange> {
    let Some(change) = change else {
        return Vec::new();
    };
    let before = terminal_mode_map(&change.before.values);
    let after = terminal_mode_map(&change.after.values);
    let keys = before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    return keys
        .into_iter()
        .filter_map(|(ansi, number)| {
            let before = before.get(&(ansi, number)).copied().unwrap_or(false);
            let after = after.get(&(ansi, number)).copied().unwrap_or(false);
            return (before != after).then_some(ActionModeChange {
                number,
                ansi,
                before,
                after,
            });
        })
        .collect();
}

fn terminal_mode_map(modes: &[TerminalMode]) -> BTreeMap<(bool, u16), bool> {
    return modes
        .iter()
        .map(|mode| return ((mode.ansi, mode.number), mode.enabled))
        .collect();
}

fn bounded_action_findings(
    findings: Vec<DiagnosticFinding>,
    from_state_id: u64,
    to_state_id: u64,
    action_token_ids: &[u64],
) -> (Vec<DiagnosticFinding>, bool) {
    let mut selected = findings
        .into_iter()
        .filter(|finding| {
            return finding
                .state_ids
                .iter()
                .any(|state_id| return *state_id > from_state_id && *state_id <= to_state_id)
                || finding
                    .token_ids
                    .iter()
                    .any(|token_id| return action_token_ids.binary_search(token_id).is_ok());
        })
        .collect::<Vec<_>>();
    let truncated = selected.len() > MAX_ACTION_FINDINGS;
    selected.truncate(MAX_ACTION_FINDINGS);
    for finding in &mut selected {
        finding.facts.truncate(MAX_FINDING_FACTS);
        finding.state_ids.truncate(MAX_FINDING_REFERENCES);
        finding.token_ids.truncate(MAX_FINDING_REFERENCES);
        finding.native_correlations.truncate(MAX_FINDING_REFERENCES);
    }
    return (selected, truncated);
}

fn action_warnings(
    rows: &[RowChange],
    before: &SemanticState,
    after: &SemanticState,
    output: &ActionOutputSummary,
    presentation_eligible_states: u64,
    broad_change_percent: u8,
    findings: &[DiagnosticFinding],
) -> (Vec<ActionWarning>, bool) {
    let changed_cells = rows.iter().fold(0_u64, |total, row| {
        return total.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
    });
    let total_cells = u64::from(after.cols.max(before.cols))
        .saturating_mul(u64::from(after.rows.max(before.rows)));
    let mut warnings = Vec::new();
    if total_cells > 0
        && changed_cells.saturating_mul(100)
            >= total_cells.saturating_mul(u64::from(broad_change_percent))
    {
        warnings.push(ActionWarning {
            code: "broad_repaint".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![format!(
                "{changed_cells} of {total_cells} viewport cells changed (threshold {broad_change_percent}%)"
            )],
        });
    }
    if output.byte_count > 0 && presentation_eligible_states == 0 {
        warnings.push(ActionWarning {
            code: "output_without_presentation".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![format!(
                "{} output bytes produced no presentation-eligible semantic boundary",
                output.byte_count
            )],
        });
    }
    if !after.cursor.in_viewport {
        warnings.push(ActionWarning {
            code: "cursor_outside_expected_region".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec!["resulting cursor is outside the viewport".to_owned()],
        });
    }
    if after.modes.synchronized_output {
        warnings.push(ActionWarning {
            code: "synchronized_output_misuse".to_owned(),
            confidence: FindingConfidence::High,
            facts: vec![
                "synchronized-output mode remained enabled at the action endpoint".to_owned(),
            ],
        });
    }
    for finding in findings {
        let code = match finding.code.as_str() {
            "presented_transient" | "clear_then_repaint" => "flicker",
            "hidden_transient" => "synchronized_output_misuse",
            _ => finding.code.as_str(),
        };
        if warnings.iter().any(|warning| return warning.code == code) {
            continue;
        }
        warnings.push(ActionWarning {
            code: code.to_owned(),
            confidence: finding.confidence,
            facts: finding
                .facts
                .iter()
                .take(MAX_FINDING_FACTS)
                .cloned()
                .collect(),
        });
    }
    let truncated = warnings.len() > MAX_ACTION_WARNINGS;
    warnings.truncate(MAX_ACTION_WARNINGS);
    return (warnings, truncated);
}
