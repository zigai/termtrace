use super::*;

pub(super) fn state_delta(previous: Option<&SemanticState>, current: &SemanticState) -> StateDelta {
    return StateDelta {
        geometry: if previous
            .is_some_and(|state| return state.cols == current.cols && state.rows == current.rows)
        {
            None
        } else {
            Some((current.cols, current.rows))
        },
        cursor: changed(previous.map(|state| return &state.cursor), &current.cursor),
        modes: changed(previous.map(|state| return &state.modes), &current.modes),
        colors: changed(previous.map(|state| return &state.colors), &current.colors),
        active_style: changed(
            previous.map(|state| return &state.active_style),
            &current.active_style,
        ),
        metadata: changed(
            previous.map(|state| return &state.metadata),
            &current.metadata,
        ),
        rows: row_diffs(previous, current),
    };
}

fn changed<T: Clone + Eq>(previous: Option<&T>, current: &T) -> Option<T> {
    return (previous != Some(current)).then(|| return current.clone());
}

fn row_diffs(previous: Option<&SemanticState>, current: &SemanticState) -> Vec<RowDiff> {
    return current
        .grid
        .iter()
        .filter_map(|row| {
            let old_row = previous.and_then(|state| return state.grid.get(usize::from(row.row)));
            let cells = row
                .cells
                .iter()
                .filter(|cell| {
                    return old_row.and_then(|old| return old.cells.get(usize::from(cell.column)))
                        != Some(*cell);
                })
                .cloned()
                .collect::<Vec<_>>();
            return (!cells.is_empty()).then_some(RowDiff {
                row: row.row,
                cells,
            });
        })
        .collect();
}
