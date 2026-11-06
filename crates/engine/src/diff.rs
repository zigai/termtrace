//! Deterministic semantic endpoint comparison.

use crate::observation::state_selector_for_session;
use crate::replay::{inspect, inspect_completed_many, inspect_many};
use crate::{Error, Result};
use std::{
    hash::{Hash, Hasher},
    path::Path,
};
use termtrace_evidence::event_log::EventLog;
pub use termtrace_model::terminal::diff::{
    CellChange, DiffEndpoint, RowChange, RowMove, ScreenChanges, ScreenRowChange, SemanticDiff,
    ValueChange,
};
use termtrace_model::{
    execution::event::Event,
    terminal::diff::row_moves,
    terminal::observation::StatePoint,
    terminal::replay::{StateInspection, StateSelector},
    terminal::screen::{ScreenCell, ScreenFrame, ScreenFrameContent, ScreenSpan},
    terminal::state::{RowState, SemanticState},
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
    let moves = semantic_row_moves(&from_inspection.state, &to_inspection.state);
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
        moves,
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

struct RowContent<'row>(&'row RowState);

impl PartialEq for RowContent<'_> {
    fn eq(&self, other: &Self) -> bool {
        return self.0.soft_wrapped == other.0.soft_wrapped
            && self.0.wrap_continuation == other.0.wrap_continuation
            && self.0.cells == other.0.cells;
    }
}

impl Eq for RowContent<'_> {}

impl Hash for RowContent<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.soft_wrapped.hash(state);
        self.0.wrap_continuation.hash(state);

        for cell in self.0.cells.iter() {
            cell.grapheme.hash(state);
        }
    }
}

fn semantic_row_moves(before: &SemanticState, after: &SemanticState) -> Vec<RowMove> {
    let before = before.grid.iter().map(RowContent).collect::<Vec<_>>();
    let after = after.grid.iter().map(RowContent).collect::<Vec<_>>();

    return row_moves(&before, &after);
}

/// Compare two frames of the same projection, reporting moved rows separately.
#[must_use]
pub fn screen_changes(before: &ScreenFrame, after: &ScreenFrame) -> ScreenChanges {
    let before_rows = frame_rows(before);
    let after_rows = frame_rows(after);
    let moves = row_moves(&before_rows, &after_rows)
        .into_iter()
        .map(|moved| {
            return RowMove {
                from_row: moved.from_row.saturating_add(before.returned_region.top),
                to_row: moved.to_row.saturating_add(after.returned_region.top),
                count: moved.count,
            };
        })
        .collect::<Vec<_>>();

    let top = before.returned_region.top.min(after.returned_region.top);
    let bottom = row_end(before, &before_rows).max(row_end(after, &after_rows));
    let rows = (top..bottom)
        .filter_map(|row| {
            let earlier = frame_row(before, &before_rows, row);
            let later = frame_row(after, &after_rows, row);
            if earlier == later || moves.iter().any(|moved| return moved.covers_later_row(row)) {
                return None;
            }

            let text = |value: Option<&FrameRow<'_>>| {
                return value.map(|row| return row.text.trim_end_matches(' ').to_owned());
            };

            return Some(ScreenRowChange {
                row,
                before: text(earlier),
                after: text(later),
                appearance_only: earlier
                    .zip(later)
                    .is_some_and(|(earlier, later)| return earlier.text == later.text),
            });
        })
        .collect();

    return ScreenChanges {
        before_state_hash: before.state_hash.clone(),
        after_state_hash: after.state_hash.clone(),
        geometry: changed(
            (before.geometry.cols, before.geometry.rows),
            (after.geometry.cols, after.geometry.rows),
        ),
        cursor: changed(before.cursor.clone(), after.cursor.clone()),
        modes: changed(before.modes.clone(), after.modes.clone()),
        moves,
        rows,
    };
}

#[derive(Clone, Copy)]
enum RowAppearance<'frame> {
    Text,
    Spans(&'frame [ScreenSpan]),
    Cells(&'frame [ScreenCell]),
}

impl PartialEq for RowAppearance<'_> {
    fn eq(&self, other: &Self) -> bool {
        return match (self, other) {
            (Self::Text, Self::Text) => true,
            (Self::Spans(earlier), Self::Spans(later)) => earlier == later,
            (Self::Cells(earlier), Self::Cells(later)) => {
                earlier.len() == later.len()
                    && earlier.iter().zip(later.iter()).all(|(earlier, later)| {
                        return earlier.column == later.column
                            && earlier.grapheme == later.grapheme
                            && earlier.wide_kind == later.wide_kind
                            && earlier.appearance == later.appearance;
                    })
            }
            _ => false,
        };
    }
}

#[derive(Clone, Copy, PartialEq)]
struct FrameRow<'frame> {
    soft_wrapped: bool,
    wrap_continuation: bool,
    text: &'frame str,
    appearance: RowAppearance<'frame>,
}

impl Eq for FrameRow<'_> {}

impl Hash for FrameRow<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.soft_wrapped.hash(state);
        self.wrap_continuation.hash(state);
        self.text.hash(state);
    }
}

fn frame_rows(frame: &ScreenFrame) -> Vec<FrameRow<'_>> {
    return match &frame.data {
        ScreenFrameContent::None => Vec::new(),
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .map(|row| {
                return FrameRow {
                    soft_wrapped: row.soft_wrapped,
                    wrap_continuation: row.wrap_continuation,
                    text: &row.text,
                    appearance: RowAppearance::Text,
                };
            })
            .collect(),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .map(|row| {
                return FrameRow {
                    soft_wrapped: row.soft_wrapped,
                    wrap_continuation: row.wrap_continuation,
                    text: &row.text,
                    appearance: RowAppearance::Spans(&row.spans),
                };
            })
            .collect(),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .map(|row| {
                return FrameRow {
                    soft_wrapped: row.soft_wrapped,
                    wrap_continuation: row.wrap_continuation,
                    text: &row.text,
                    appearance: RowAppearance::Cells(&row.cells),
                };
            })
            .collect(),
    };
}

fn frame_row<'rows, 'frame>(
    frame: &ScreenFrame,
    rows: &'rows [FrameRow<'frame>],
    row: u16,
) -> Option<&'rows FrameRow<'frame>> {
    let index = row.checked_sub(frame.returned_region.top)?;

    return rows.get(usize::from(index));
}

fn row_end(frame: &ScreenFrame, rows: &[FrameRow<'_>]) -> u16 {
    let count = u16::try_from(rows.len()).unwrap_or(u16::MAX);

    return frame.returned_region.top.saturating_add(count);
}

fn diff_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
        source: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_model::{
        execution::event_log::CanonicalBoundary,
        terminal::observation::{ActorBoundaries, CaptureQuality},
        terminal::screen::{FrameDetail, ScreenFrameSource, ScreenProjectionRequest, ScreenRegion},
    };
    use termtrace_terminal::{project_screen_frame, GhosttyVt};

    fn frame(terminal: &mut GhosttyVt, detail: FrameDetail) -> ScreenFrame {
        let state = terminal.snapshot().expect("snapshot");
        let boundary = CanonicalBoundary {
            next_event_seq: 1,
            event_hash: "a".repeat(64),
            output_bytes: 0,
        };
        let source = ScreenFrameSource::Actor {
            session_id: Default::default(),
            boundaries: ActorBoundaries {
                durable: boundary.clone(),
                observed: boundary,
            },
            observed_monotonic_ns: 0,
            state_id: 0,
        };

        let region = (detail == FrameDetail::Cells).then_some(ScreenRegion {
            top: 0,
            left: 0,
            right: state.cols,
            bottom: state.rows,
        });

        let request = ScreenProjectionRequest { detail, region };

        return project_screen_frame(&state, &request, source, true, CaptureQuality::Live)
            .expect("frame");
    }

    fn spans(terminal: &mut GhosttyVt) -> ScreenFrame {
        return frame(terminal, FrameDetail::Spans);
    }

    #[test]
    fn identical_frames_report_no_changes() {
        let mut terminal = GhosttyVt::new(8, 3, 0).expect("terminal");
        terminal.write(b"same");
        let before = spans(&mut terminal);
        let changes = screen_changes(&before, &spans(&mut terminal));
        assert!(changes.is_empty());
        assert_eq!(changes.before_state_hash, changes.after_state_hash);
    }

    #[test]
    fn edited_row_reports_its_text_and_the_cursor_move() {
        let mut terminal = GhosttyVt::new(8, 3, 0).expect("terminal");
        terminal.write(b"name:");
        let before = spans(&mut terminal);
        terminal.write(b"ab");
        let changes = screen_changes(&before, &spans(&mut terminal));
        assert_eq!(
            changes.rows,
            [ScreenRowChange {
                row: 0,
                before: Some("name:".to_owned()),
                after: Some("name:ab".to_owned()),
                appearance_only: false,
            }]
        );
        assert!(changes.cursor.is_some());
        assert!(changes.moves.is_empty());
    }

    #[test]
    fn style_only_change_is_marked_appearance_only() {
        let mut terminal = GhosttyVt::new(8, 2, 0).expect("terminal");
        terminal.write(b"item");
        let before = spans(&mut terminal);
        terminal.write(b"\r\x1b[7mitem");
        let changes = screen_changes(&before, &spans(&mut terminal));
        assert_eq!(changes.rows.len(), 1);
        assert!(changes.rows[0].appearance_only);
    }

    #[test]
    fn scrolled_rows_are_moves_and_only_new_rows_are_listed() {
        let mut terminal = GhosttyVt::new(8, 3, 0).expect("terminal");
        terminal.write(b"one\r\ntwo\r\nthree");
        let before = spans(&mut terminal);
        terminal.write(b"\r\nfour");
        let changes = screen_changes(&before, &spans(&mut terminal));
        assert_eq!(
            changes.moves,
            [RowMove {
                from_row: 1,
                to_row: 0,
                count: 2,
            }]
        );
        assert_eq!(
            changes.rows,
            [ScreenRowChange {
                row: 2,
                before: Some("three".to_owned()),
                after: Some("four".to_owned()),
                appearance_only: false,
            }]
        );
    }

    #[test]
    fn cell_frames_detect_moves_despite_absolute_row_numbers() {
        let mut terminal = GhosttyVt::new(8, 3, 0).expect("terminal");
        terminal.write(b"one\r\ntwo\r\nthree");
        let before = frame(&mut terminal, FrameDetail::Cells);
        terminal.write(b"\r\nfour");
        let changes = screen_changes(&before, &frame(&mut terminal, FrameDetail::Cells));
        assert_eq!(
            changes.moves,
            [RowMove {
                from_row: 1,
                to_row: 0,
                count: 2,
            }]
        );
    }

    #[test]
    fn rows_missing_from_one_frame_report_an_absent_side() {
        let mut short = GhosttyVt::new(8, 1, 0).expect("terminal");
        short.write(b"top");

        let mut tall = GhosttyVt::new(8, 2, 0).expect("terminal");
        tall.write(b"top\r\nnext");
        let changes = screen_changes(&spans(&mut short), &spans(&mut tall));
        assert!(changes.geometry.is_some());
        assert_eq!(
            changes.rows,
            [ScreenRowChange {
                row: 1,
                before: None,
                after: Some("next".to_owned()),
                appearance_only: false,
            }]
        );
    }

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

    #[test]
    fn scrolled_rows_are_reported_as_one_move() {
        let mut terminal = GhosttyVt::new(8, 3, 0).expect("terminal");
        terminal.write(b"one\r\ntwo\r\nthree");
        let before = terminal.snapshot().expect("before");
        terminal.write(b"\r\nfour");
        let after = terminal.snapshot().expect("after");
        assert_eq!(
            semantic_row_moves(&before, &after),
            [RowMove {
                from_row: 1,
                to_row: 0,
                count: 2,
            }]
        );
    }
}
