//! Stable deterministic semantic endpoint comparison contracts.

use crate::{
    terminal::observation::StatePoint,
    terminal::state::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, ModeState, TerminalMetadata,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, hash::Hash};

/// Stable identity for one semantic diff endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffEndpoint {
    /// Requested endpoint selector.
    pub point: StatePoint,
    /// Stream position of the state.
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
    /// Blocks of complete rows that moved; `rows` still compares cells at equal positions.
    pub moves: Vec<RowMove>,
}

/// Presented-screen changes between two frames of the same projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenChanges {
    /// State hash of the earlier frame.
    pub before_state_hash: String,
    /// State hash of the later frame.
    pub after_state_hash: String,
    /// Geometry change as columns and rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<ValueChange<(u16, u16)>>,
    /// Cursor change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ValueChange<CursorState>>,
    /// Terminal mode change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ValueChange<ModeState>>,
    /// Blocks of complete rows that moved.
    pub moves: Vec<RowMove>,
    /// Rows that differ at the same position and hold no moved content.
    pub rows: Vec<ScreenRowChange>,
}

impl ScreenChanges {
    /// Whether any reported property or row changed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        return self.geometry.is_none()
            && self.cursor.is_none()
            && self.modes.is_none()
            && self.moves.is_empty()
            && self.rows.is_empty();
    }
}

/// One changed row of a presented-screen frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenRowChange {
    /// Absolute viewport row.
    pub row: u16,
    /// Earlier row text without trailing blanks, absent when the earlier frame had no such row.
    pub before: Option<String>,
    /// Later row text without trailing blanks, absent when the later frame has no such row.
    pub after: Option<String>,
    /// Whether the text is unchanged and only appearance or wrap facts differ.
    pub appearance_only: bool,
}

/// Contiguous block of complete rows found at a different position in the later state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowMove {
    /// First row of the block in the earlier state.
    pub from_row: u16,
    /// First row of the block in the later state.
    pub to_row: u16,
    /// Number of rows in the block.
    pub count: u16,
}

impl RowMove {
    /// Whether `row` of the later state holds content moved by this block.
    #[must_use]
    pub const fn covers_later_row(&self, row: u16) -> bool {
        return row >= self.to_row && row - self.to_row < self.count;
    }
}

/// Find blocks of rows that moved between two row sequences.
///
/// Rows that occur exactly once in both sequences anchor a match, and matches spread to
/// equal neighbors, so repeated rows such as blanks move only alongside unique content
/// (Heckel, "A Technique for Isolating Differences Between Files", CACM 1978). Rows that
/// are also equal at their own position are trimmed from both ends of each block.
#[must_use]
pub fn row_moves<T: Eq + Hash>(before: &[T], after: &[T]) -> Vec<RowMove> {
    let mut links = unique_row_links(before, after);

    let mut linked_before = vec![false; before.len()];
    for index in links.iter().flatten() {
        if let Some(linked) = linked_before.get_mut(*index) {
            *linked = true;
        }
    }

    for later in 0..after.len().saturating_sub(1) {
        let next = links
            .get(later)
            .copied()
            .flatten()
            .map(|earlier| return earlier + 1);
        link_equal_neighbor(
            &mut links,
            &mut linked_before,
            before,
            after,
            later + 1,
            next,
        );
    }

    for later in (1..after.len()).rev() {
        let previous = links
            .get(later)
            .copied()
            .flatten()
            .and_then(|earlier| return earlier.checked_sub(1));
        link_equal_neighbor(
            &mut links,
            &mut linked_before,
            before,
            after,
            later - 1,
            previous,
        );
    }

    let link = |later: usize| return links.get(later).copied().flatten();
    let unmoved = |row: usize| return before.get(row) == after.get(row);
    let mut moves = Vec::new();

    let mut later = 0;
    while later < after.len() {
        let Some(earlier) = link(later) else {
            later += 1;
            continue;
        };

        let mut count = 1;
        while link(later + count) == Some(earlier + count) {
            count += 1;
        }

        if earlier != later {
            let mut start = 0;
            let mut end = count;

            while start < end && unmoved(later + start) {
                start += 1;
            }

            while end > start && unmoved(later + end - 1) {
                end -= 1;
            }

            if start < end {
                moves.push(RowMove {
                    from_row: saturating_row(earlier + start),
                    to_row: saturating_row(later + start),
                    count: saturating_row(end - start),
                });
            }
        }

        later += count;
    }

    return moves;
}

fn link_equal_neighbor<T: Eq>(
    links: &mut [Option<usize>],
    linked_before: &mut [bool],
    before: &[T],
    after: &[T],
    later: usize,
    earlier: Option<usize>,
) {
    let Some(earlier) = earlier else {
        return;
    };
    let (Some(link @ None), Some(linked @ false)) =
        (links.get_mut(later), linked_before.get_mut(earlier))
    else {
        return;
    };

    if before.get(earlier) == after.get(later) {
        *link = Some(earlier);
        *linked = true;
    }
}

fn unique_row_links<T: Eq + Hash>(before: &[T], after: &[T]) -> Vec<Option<usize>> {
    let mut occurrences: HashMap<&T, (usize, usize, usize)> = HashMap::new();
    for (index, row) in before.iter().enumerate() {
        occurrences.entry(row).or_insert((0, index, 0)).0 += 1;
    }

    for row in after {
        if let Some(entry) = occurrences.get_mut(row) {
            entry.2 += 1;
        }
    }

    return after
        .iter()
        .map(|row| {
            return occurrences
                .get(row)
                .and_then(|&(before_count, index, after_count)| {
                    return (before_count == 1 && after_count == 1).then_some(index);
                });
        })
        .collect();
}

fn saturating_row(value: usize) -> u16 {
    return u16::try_from(value).unwrap_or(u16::MAX);
}

#[cfg(test)]
mod tests {
    use super::{row_moves, RowMove};

    fn moves(before: &[&str], after: &[&str]) -> Vec<RowMove> {
        return row_moves(before, after);
    }

    fn block(from_row: u16, to_row: u16, count: u16) -> RowMove {
        return RowMove {
            from_row,
            to_row,
            count,
        };
    }

    #[test]
    fn unchanged_rows_report_no_moves() {
        let rows = ["a", "", "b", ""];
        assert_eq!(moves(&rows, &rows), []);
    }

    #[test]
    fn scrolling_up_reports_one_block_with_its_offset() {
        let before = ["one", "two", "three", "four", "five"];
        let after = ["three", "four", "five", "six", "seven"];
        assert_eq!(moves(&before, &after), [block(2, 0, 3)]);
    }

    #[test]
    fn inserted_row_moves_the_rows_below_it() {
        let before = ["head", "a", "b", "c"];
        let after = ["head", "new", "a", "b"];
        assert_eq!(moves(&before, &after), [block(1, 2, 2)]);
    }

    #[test]
    fn repeated_rows_alone_never_form_a_move() {
        let before = ["", "x", "", "-", "-"];
        let after = ["", "", "y", "-", "-"];
        assert_eq!(moves(&before, &after), []);
    }

    #[test]
    fn repeated_rows_move_alongside_unique_neighbors() {
        let before = ["title", "", "body", "", "", ""];
        let after = ["", "", "title", "", "body", ""];
        assert_eq!(moves(&before, &after), [block(0, 2, 3)]);
    }

    #[test]
    fn rows_equal_at_their_own_position_are_trimmed_from_block_ends() {
        let before = ["mark", "", "", "", "end"];
        let after = ["", "mark", "", "", "end"];
        assert_eq!(moves(&before, &after), [block(0, 1, 1)]);
    }

    #[test]
    fn swapped_blocks_report_two_moves() {
        let before = ["a1", "a2", "b1", "b2"];
        let after = ["b1", "b2", "a1", "a2"];
        assert_eq!(moves(&before, &after), [block(2, 0, 2), block(0, 2, 2)]);
    }

    #[test]
    fn sequences_of_different_lengths_compare_their_shared_content() {
        let before = ["a", "b", "c"];
        let after = ["x", "a", "b", "c", "d"];
        assert_eq!(moves(&before, &after), [block(0, 1, 3)]);
    }

    #[test]
    fn row_duplicated_in_the_later_state_is_not_an_anchor() {
        let before = ["a", "b"];
        let after = ["b", "a", "a"];
        assert_eq!(moves(&before, &after), [block(1, 0, 1)]);
    }

    #[test]
    fn covers_later_row_spans_exactly_the_destination_block() {
        let moved = block(5, 2, 3);
        assert!(!moved.covers_later_row(1));
        assert!(moved.covers_later_row(2));
        assert!(moved.covers_later_row(4));
        assert!(!moved.covers_later_row(5));
    }
}
