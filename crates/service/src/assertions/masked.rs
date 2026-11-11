use super::{
    data_for_profile, normalize_frame, AssertionError, QueryError, ScreenFrame, ScreenFrameContent,
    ScreenFrameError, ScreenRegion, ScreenSnapshotOptions, SnapshotProfile, WideKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use termtrace_model::terminal::screen::{MAX_SCREEN_FRAME_CELLS, SCREEN_FRAME_VERSION};

/// Options for the separate version-one cells comparison format.
///
/// Masks exclude unstable comparison cells. They do not redact canonical evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaskedSnapshotOptions {
    /// Only `cells` is supported. Omission selects cells.
    #[serde(default = "cells_profile")]
    pub profile: SnapshotProfile,
    /// Nonempty, absolute, half-open rectangles inside the returned region.
    #[serde(default)]
    pub mask_regions: Vec<ScreenRegion>,
    /// Compare cursor facts too.
    #[serde(default)]
    pub include_cursor: bool,
    /// Compare terminal modes too.
    #[serde(default)]
    pub include_modes: bool,
}

impl Default for MaskedSnapshotOptions {
    fn default() -> Self {
        return Self {
            profile: SnapshotProfile::Cells,
            mask_regions: Vec::new(),
            include_cursor: false,
            include_modes: false,
        };
    }
}

fn cells_profile() -> SnapshotProfile {
    return SnapshotProfile::Cells;
}

fn invalid(reason: &str) -> AssertionError {
    return AssertionError::Query(QueryError::InvalidQuery {
        reason: reason.to_owned(),
    });
}

/// Normalize an immutable exact frame into `termtrace-masked-snapshot-v1`.
///
/// Row text is deliberately omitted. Geometry, wraps, colors and capture quality remain.
/// The union is canonicalized independently of rectangle order, overlap or decomposition.
///
/// # Errors
/// Rejects non-cells profiles, missing/inconsistent width maps, invalid rectangles and masks
/// splitting represented wide graphemes (including wrap-edge spacer heads).
pub fn masked_snapshot(
    frame: &ScreenFrame,
    options: &MaskedSnapshotOptions,
) -> Result<String, AssertionError> {
    if frame.version != SCREEN_FRAME_VERSION {
        return Err(invalid(
            "unsupported screen frame version for masked comparison",
        ));
    }

    if options.profile != SnapshotProfile::Cells {
        return Err(invalid("masked comparisons support only the cells profile"));
    }

    if let ScreenFrameContent::Spans { rows } = &frame.data {
        for row in rows {
            for span in &row.spans {
                if usize::from(span.cell_count) != span.cells.len()
                    || span.end_column.checked_sub(span.start_column) != Some(span.cell_count)
                {
                    return Err(invalid("inconsistent span width map"));
                }
            }
        }
    }

    let ScreenFrameContent::Cells { rows } =
        data_for_profile(frame, SnapshotProfile::Cells).map_err(AssertionError::Snapshot)?
    else {
        return Err(invalid("masked comparisons require exact cell width maps"));
    };

    let region = frame.returned_region;

    if region.width() == 0
        || region.height() == 0
        || region.bottom > frame.geometry.rows
        || region.right > frame.geometry.cols
    {
        return Err(invalid("invalid returned region"));
    }

    if u32::from(region.width()) * u32::from(region.height()) > MAX_SCREEN_FRAME_CELLS {
        return Err(invalid("masked comparison exceeds the frame cell limit"));
    }

    let mut masked = BTreeSet::new();

    for mask in &options.mask_regions {
        if mask.width() == 0
            || mask.height() == 0
            || mask.top < region.top
            || mask.left < region.left
            || mask.bottom > region.bottom
            || mask.right > region.right
        {
            return Err(invalid(
                "mask must be nonempty and wholly inside the returned region",
            ));
        }

        for row in mask.top..mask.bottom {
            for column in mask.left..mask.right {
                masked.insert((row, column));
            }
        }
    }

    if rows.len() != usize::from(region.height()) {
        return Err(invalid("incomplete masked-comparison row map"));
    }

    for (offset, row) in rows.iter().enumerate() {
        if usize::from(row.row) != usize::from(region.top) + offset
            || row.cells.len() != usize::from(region.width())
        {
            return Err(invalid("incomplete masked-comparison cell map"));
        }

        for (offset, cell) in row.cells.iter().enumerate() {
            if cell.row != row.row || usize::from(cell.column) != usize::from(region.left) + offset
            {
                return Err(invalid("inconsistent masked-comparison cell coordinates"));
            }

            let partner = match cell.wide_kind {
                WideKind::Wide => {
                    Some((row.row, cell.column.saturating_add(1), WideKind::SpacerTail))
                }
                WideKind::SpacerTail => {
                    Some((row.row, cell.column.saturating_sub(1), WideKind::Wide))
                }
                WideKind::SpacerHead => Some((row.row.saturating_add(1), 0, WideKind::Wide)),
                WideKind::Narrow => None,
            };

            if let Some((other_row, column, kind)) = partner {
                let other = rows
                    .iter()
                    .find(|row| return row.row == other_row)
                    .and_then(|row| {
                        return row.cells.iter().find(|cell| return cell.column == column);
                    });
                if let Some(other) = other {
                    if other.wide_kind != kind {
                        return Err(invalid("inconsistent wide-cell map"));
                    }

                    if masked.contains(&(row.row, cell.column))
                        != masked.contains(&(other_row, column))
                    {
                        return Err(invalid("mask splits a represented wide grapheme"));
                    }
                } else if masked.contains(&(row.row, cell.column)) {
                    return Err(invalid(
                        "masked wide grapheme extends outside the returned region",
                    ));
                }
            }
        }
    }

    let mut canonical: Vec<ScreenRegion> = Vec::new();

    for row in region.top..region.bottom {
        let mut column = region.left;

        while column < region.right {
            if !masked.contains(&(row, column)) {
                column += 1;
                continue;
            }

            let left = column;

            while column < region.right && masked.contains(&(row, column)) {
                column += 1;
            }

            if let Some(previous) = canonical
                .iter_mut()
                .find(|r| return r.bottom == row && r.left == left && r.right == column)
            {
                previous.bottom += 1;
            } else {
                canonical.push(ScreenRegion {
                    top: row,
                    bottom: row + 1,
                    left,
                    right: column,
                });
            }
        }
    }

    let mut value = normalize_frame(
        frame,
        SnapshotProfile::Cells,
        ScreenSnapshotOptions {
            include_cursor: options.include_cursor,
            include_modes: options.include_modes,
        },
    )
    .map_err(AssertionError::Snapshot)?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| return invalid("invalid normalized comparison document"))?;
    fields.insert(
        "format".to_owned(),
        serde_json::json!("termtrace-masked-snapshot-v1"),
    );
    fields.insert("version".to_owned(), serde_json::json!(1));
    fields.insert("mask_regions".to_owned(), serde_json::json!(canonical));
    if let Some(rows) = value.get_mut("rows").and_then(Value::as_array_mut) {
        for row in rows {
            let row_number = row.get("row").and_then(Value::as_u64).unwrap_or(u64::MAX);

            if let Some(object) = row.as_object_mut() {
                object.remove("text");
            }

            if let Some(cells) = row.get_mut("cells").and_then(Value::as_array_mut) {
                for cell in cells {
                    let column = cell
                        .get("column")
                        .and_then(Value::as_u64)
                        .unwrap_or(u64::MAX);

                    if let (Ok(row), Ok(col)) = (u16::try_from(row_number), u16::try_from(column))
                        && masked.contains(&(row, col))
                    {
                        *cell = serde_json::json!({"column": col, "masked": true});
                    }
                }
            }
        }
    }

    return serde_json::to_string_pretty(&value)
        .map_err(ScreenFrameError::serialization)
        .map_err(AssertionError::Snapshot);
}
