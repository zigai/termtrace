//! Deterministic semantic endpoint comparison.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::Event,
    event_log::EventLog,
    ghostty_vt::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, ModeState, SemanticState,
        TerminalMetadata,
    },
    observation::StatePoint,
    replay::{StateInspection, StateSelector, inspect, inspect_many},
};

/// Stable identity for one semantic diff endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffEndpoint {
    /// Requested endpoint selector.
    pub point: StatePoint,
    /// Dense semantic state identifier.
    pub state_id: u64,
    /// Complete semantic state hash.
    pub state_hash: String,
    /// Responsible event sequence.
    pub event_seq: Option<u64>,
    /// Session-monotonic timestamp.
    pub monotonic_ns: u64,
    /// Whether the state was presentation eligible.
    pub presented: bool,
    /// Ghostty dirty classification at this endpoint.
    pub ghostty_dirty: DirtyState,
    /// Rows Ghostty marked dirty at this endpoint.
    pub ghostty_dirty_rows: Vec<u16>,
}

/// Before/after values for one changed semantic property.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValueChange<T> {
    /// Value at the earlier endpoint.
    pub before: T,
    /// Value at the later endpoint.
    pub after: T,
}

/// One changed viewport cell.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CellChange {
    /// Zero-based column.
    pub column: u16,
    /// Complete earlier cell, absent outside earlier geometry.
    pub before: Option<CellState>,
    /// Complete later cell, absent outside later geometry.
    pub after: Option<CellState>,
}

/// Changed cells and native wrap facts for one viewport row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowChange {
    /// Zero-based row.
    pub row: u16,
    /// Earlier/later soft-wrap fact, including row absence at either endpoint.
    pub soft_wrapped: Option<ValueChange<Option<bool>>>,
    /// Earlier/later wrap-continuation fact, including row absence at either endpoint.
    pub wrap_continuation: Option<ValueChange<Option<bool>>>,
    /// Complete semantic cell changes.
    pub cells: Vec<CellChange>,
}

/// Complete semantic difference between two canonical state points.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticDiff {
    /// Earlier endpoint identity and provenance.
    pub from: DiffEndpoint,
    /// Later endpoint identity and provenance.
    pub to: DiffEndpoint,
    /// Whether any normalized semantic or presentation value changed.
    pub changed: bool,
    /// Geometry change.
    pub geometry: Option<ValueChange<(u16, u16)>>,
    /// Cursor change.
    pub cursor: Option<ValueChange<CursorState>>,
    /// Terminal mode change.
    pub modes: Option<ValueChange<ModeState>>,
    /// Palette/default-color change.
    pub colors: Option<ValueChange<ColorState>>,
    /// Active SGR style change.
    pub active_style: Option<ValueChange<CellStyle>>,
    /// OSC metadata change.
    pub metadata: Option<ValueChange<TerminalMetadata>>,
    /// Presentation-eligibility change.
    pub presented: Option<ValueChange<bool>>,
    /// Minimal changed semantic cells grouped by row.
    pub rows: Vec<RowChange>,
}

/// Compare two points reconstructed from the same validated event log.
///
/// # Errors
///
/// Returns an error when either selector is missing/ambiguous or replay fails.
pub fn diff_states(log: &EventLog, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
    let (difference, _, _) = diff_states_with_semantic_states(log, from, to)?;
    return Ok(difference);
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
    let mut inspections = inspect_points(log, &[from.clone(), to.clone()])?;
    let to_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing later diff endpoint"))?;
    let from_inspection = inspections
        .pop()
        .ok_or_else(|| return diff_error("missing earlier diff endpoint"))?;
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
    return Ok((difference, from_inspection.state, to_inspection.state));
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

/// Resolve several shared state points in one canonical replay.
///
/// # Errors
///
/// Returns an error when replay fails or any marker/selector cannot resolve.
pub fn inspect_points(log: &EventLog, points: &[StatePoint]) -> Result<Vec<StateInspection>> {
    let selectors = points
        .iter()
        .map(|point| return state_selector(log, point))
        .collect::<Result<Vec<_>>>()?;
    return inspect_many(log, &selectors);
}

fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
    return Ok(match point {
        StatePoint::LatestPresented => StateSelector::LatestPresented,
        StatePoint::StateId { state_id } => StateSelector::StateId(*state_id),
        StatePoint::StateHash { state_hash } => StateSelector::StateHash(state_hash.clone()),
        StatePoint::EventSeq { event_seq } => StateSelector::EventSeq(*event_seq),
        StatePoint::MonotonicNs { monotonic_ns } => StateSelector::MonotonicNs(*monotonic_ns),
        StatePoint::Marker { label } => StateSelector::EventSeq(unique_marker(log, label)?),
    });
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
        let before_row = before.grid.iter().find(|value| return value.row == row);
        let after_row = after.grid.iter().find(|value| return value.row == row);
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
            let before_cell = before_row.and_then(|value| {
                return value.cells.iter().find(|cell| return cell.column == column);
            });
            let after_cell = after_row.and_then(|value| {
                return value.cells.iter().find(|cell| return cell.column == column);
            });
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
    use crate::ghostty_vt::GhosttyVt;

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
