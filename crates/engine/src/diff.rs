//! Deterministic semantic endpoint comparison.

use crate::observation::state_selector_for_session;
use crate::replay::{inspect, inspect_completed_many, inspect_many};
use crate::{Error, Result};
use std::path::Path;
use termtrace_evidence::event_log::EventLog;
pub use termtrace_model::terminal::diff::{
    CellChange, DiffEndpoint, RowChange, SemanticDiff, ValueChange,
};
use termtrace_model::{
    execution::event::Event,
    terminal::observation::StatePoint,
    terminal::replay::{StateInspection, StateSelector},
    terminal::state::SemanticState,
};

/// Compare two points reconstructed from the same validated event log.
///
/// # Errors
///
/// Returns an error when either selector is missing/ambiguous or replay fails.
pub fn diff_states(log: &EventLog, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
    let (difference, _, _) = diff_states_with_semantic_states(log, from, to)?;

    return Ok(difference);
}

/// Compare completed session endpoints without retaining canonical history.
///
/// # Errors
/// Rejects incomplete/changing evidence, missing/ambiguous selectors, and replay failures.
pub fn diff_session_states_with_semantic_states(
    session: &Path,
    from: StatePoint,
    to: StatePoint,
) -> Result<(SemanticDiff, SemanticState, SemanticState)> {
    validate_diff_endpoints(&from, &to)?;
    let selectors = [
        state_selector_for_session(session, &from, None)?,
        state_selector_for_session(session, &to, None)?,
    ];
    let mut inspections = inspect_completed_many(session, &selectors)?;
    let to_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing later diff endpoint"))?;
    let from_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing earlier diff endpoint"))?;

    if from_inspection.record.state_id > to_inspection.record.state_id {
        return Err(diff_error(format!(
            "inverted diff endpoints: earlier endpoint resolved to state {} but later endpoint resolved to state {}",
            from_inspection.record.state_id, to_inspection.record.state_id
        )));
    }
    return Ok(diff_inspections_with_semantic_states(
        from,
        to,
        from_inspection,
        to_inspection,
    ));
}

/// Compare two points and retain both complete endpoint states from the same replay.
///
/// # Errors
///
/// Returns an error when either selector is missing/ambiguous or replay fails.
pub fn diff_states_with_semantic_states(
    log: &EventLog,
    from: StatePoint,
    to: StatePoint,
) -> Result<(SemanticDiff, SemanticState, SemanticState)> {
    validate_diff_endpoints(&from, &to)?;
    let mut inspections = inspect_points(log, &[from.clone(), to.clone()])?;
    let to_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing later diff endpoint"))?;
    let from_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing earlier diff endpoint"))?;

    if from_inspection.record.state_id > to_inspection.record.state_id {
        return Err(diff_error(format!(
            "inverted diff endpoints: earlier endpoint resolved to state {} but later endpoint resolved to state {}",
            from_inspection.record.state_id, to_inspection.record.state_id
        )));
    }
    return Ok(diff_inspections_with_semantic_states(
        from,
        to,
        from_inspection,
        to_inspection,
    ));
}

fn validate_diff_endpoints(from: &StatePoint, to: &StatePoint) -> Result<()> {
    match (from, to) {
        (
            StatePoint::EventSeq {
                event_seq: from_seq,
            },
            StatePoint::EventSeq { event_seq: to_seq },
        ) if from_seq > to_seq => {
            return Err(diff_error(format!(
                "inverted diff range: from-event-seq ({from_seq}) > to-event-seq ({to_seq})"
            )));
        }
        (StatePoint::StateId { state_id: from_id }, StatePoint::StateId { state_id: to_id })
            if from_id > to_id =>
        {
            return Err(diff_error(format!(
                "inverted diff range: from-state ({from_id}) > to-state ({to_id})"
            )));
        }
        (
            StatePoint::MonotonicNs {
                monotonic_ns: from_ns,
            },
            StatePoint::MonotonicNs {
                monotonic_ns: to_ns,
            },
        ) if from_ns > to_ns => {
            return Err(diff_error(format!(
                "inverted diff range: from-at ({from_ns}) > to-at ({to_ns})"
            )));
        }
        _ => return Ok(()),
    }
}

/// Diff two already-replayed state inspections directly without re-reading or replaying the log.
pub fn diff_inspections_with_semantic_states(
    from: StatePoint,
    to: StatePoint,
    from_inspection: StateInspection,
    to_inspection: StateInspection,
) -> (SemanticDiff, SemanticState, SemanticState) {
    let rows = changed_rows(&from_inspection.state, &to_inspection.state);
    let geometry = changed(
        (from_inspection.state.cols, from_inspection.state.rows),
        (to_inspection.state.cols, to_inspection.state.rows),
    );

    let cursor = changed(
        from_inspection.state.cursor.clone(),
        to_inspection.state.cursor.clone(),
    );

    let modes = changed(
        from_inspection.state.modes.clone(),
        to_inspection.state.modes.clone(),
    );

    let colors = changed(
        from_inspection.state.colors.clone(),
        to_inspection.state.colors.clone(),
    );

    let active_style = changed(
        from_inspection.state.active_style.clone(),
        to_inspection.state.active_style.clone(),
    );

    let metadata = changed(
        from_inspection.state.metadata.clone(),
        to_inspection.state.metadata.clone(),
    );

    let presented = changed(
        from_inspection.record.presented,
        to_inspection.record.presented,
    );

    let any_changed = geometry.is_some()
        || cursor.is_some()
        || modes.is_some()
        || colors.is_some()
        || active_style.is_some()
        || metadata.is_some()
        || presented.is_some()
        || !rows.is_empty();

    let difference = SemanticDiff {
        from: endpoint(from, &from_inspection),
        to: endpoint(to, &to_inspection),
        changed: any_changed,
        geometry,
        cursor,
        modes,
        colors,
        active_style,
        metadata,
        presented,
        rows,
    };

    return (difference, from_inspection.state, to_inspection.state);
}

/// Resolve one shared state point against a canonical event log.
///
/// # Errors
///
/// Returns an error when replay fails or a marker is missing/ambiguous.
pub fn inspect_point(log: &EventLog, point: &StatePoint) -> Result<StateInspection> {
    let selector = state_selector(log, point)?;

    return inspect(log, selector);
}

fn inspect_points(log: &EventLog, points: &[StatePoint]) -> Result<Vec<StateInspection>> {
    let selectors = points
        .iter()
        .map(|point| return state_selector(log, point))
        .collect::<Result<Vec<_>>>()?;

    return inspect_many(log, &selectors);
}

fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
    return point.selector(|label| return unique_marker(log, label));
}

fn unique_marker(log: &EventLog, label: &str) -> Result<u64> {
    let mut matches = log.events.iter().filter(|record| {
        return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
    });
    let sequence = matches
        .next()
        .map(|record| return record.seq)
        .ok_or_else(|| return diff_error("marker does not exist"))?;

    if matches.next().is_some() {
        return Err(diff_error("marker label is ambiguous"));
    }
    return Ok(sequence);
}

fn endpoint(point: StatePoint, inspection: &StateInspection) -> DiffEndpoint {
    return DiffEndpoint {
        point,
        state_id: inspection.record.state_id,
        state_hash: inspection.record.state_hash.clone(),
        event_seq: inspection.record.event_seq,
        monotonic_ns: inspection.record.monotonic_ns,
        presented: inspection.record.presented,
        ghostty_dirty: inspection.record.ghostty_dirty,
        ghostty_dirty_rows: inspection.record.ghostty_dirty_rows.clone(),
    };
}

fn changed<T: Eq>(before: T, after: T) -> Option<ValueChange<T>> {
    return (before != after).then_some(ValueChange { before, after });
}

fn changed_rows(before: &SemanticState, after: &SemanticState) -> Vec<RowChange> {
    let rows = before.rows.max(after.rows);
    let columns = before.cols.max(after.cols);
    let mut row_changes = Vec::new();

    for row in 0..rows {
        let before_row = before.grid.get(usize::from(row));
        let after_row = after.grid.get(usize::from(row));
        let soft_wrapped = changed(
            before_row.map(|value| return value.soft_wrapped),
            after_row.map(|value| return value.soft_wrapped),
        );

        let wrap_continuation = changed(
            before_row.map(|value| return value.wrap_continuation),
            after_row.map(|value| return value.wrap_continuation),
        );

        let mut cells = Vec::new();
        for column in 0..columns {
            let before_cell =
                before_row.and_then(|value| return value.cells.get(usize::from(column)));

            let after_cell =
                after_row.and_then(|value| return value.cells.get(usize::from(column)));

            if before_cell != after_cell {
                cells.push(CellChange {
                    column,
                    before: before_cell.cloned(),
                    after: after_cell.cloned(),
                });
            }
        }

        if !cells.is_empty() || soft_wrapped.is_some() || wrap_continuation.is_some() {
            row_changes.push(RowChange {
                row,
                soft_wrapped,
                wrap_continuation,
                cells,
            });
        }
    }

    return row_changes;
}

fn diff_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_terminal::GhosttyVt;

    #[test]
    fn changed_rows_retains_wrap_only_changes() {
        let mut terminal = GhosttyVt::new(4, 2, 0).expect("terminal");
        terminal.write(b"abcd");
        let before = terminal.snapshot().expect("before");
        let mut after = before.clone();
        after.grid[0].soft_wrapped = !after.grid[0].soft_wrapped;
        after.grid[1].wrap_continuation = !after.grid[1].wrap_continuation;
        let rows = changed_rows(&before, &after);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| return row.cells.is_empty()));
        assert!(rows[0].soft_wrapped.is_some());
        assert!(rows[1].wrap_continuation.is_some());
    }
}
