//! Bounded rich projections of authoritative semantic terminal state.

use serde::Serialize;
use std::io::{self, Write};
pub use termtrace_model::terminal::screen::*;
use termtrace_model::{
    terminal::observation::CaptureQuality,
    terminal::state::{
        CellState, CellStyle, ColorState, CursorState, ModeState, RowState, SemanticState,
        TerminalMetadata, WideKind,
    },
};

const SCREEN_FRAME_HASH_DOMAIN: &[u8] = b"termtrace-screen-frame-v1\0";
const SEMANTIC_STATE_HASH_DOMAIN: &[u8] = b"termtrace-semantic-state-v2\0";

#[derive(Serialize)]
struct StateHashMetadata<'state> {
    cols: u16,
    rows: u16,
    cursor: &'state CursorState,
    modes: &'state ModeState,
    colors: &'state ColorState,
    active_style: &'state CellStyle,
    metadata: &'state TerminalMetadata,
}

#[derive(Serialize)]
struct StateHashRow<'state> {
    row: u16,
    soft_wrapped: bool,
    wrap_continuation: bool,
    cells: &'state [CellState],
}

/// Hash normalized semantic terminal state using the stable v2 contract.
pub fn semantic_state_hash(state: &SemanticState) -> Result<String, ScreenFrameError> {
    let mut hash = semantic_state_metadata_hasher(state)?;
    for row in &state.grid {
        hash.update(hash_semantic_row(row)?.as_bytes());
    }

    return Ok(hash.finalize().to_hex().to_string());
}

/// Hash one semantic row, excluding transient renderer dirtiness.
pub fn hash_semantic_row(row: &RowState) -> Result<blake3::Hash, ScreenFrameError> {
    let normalized = StateHashRow {
        row: row.row,
        soft_wrapped: row.soft_wrapped,
        wrap_continuation: row.wrap_continuation,
        cells: &row.cells,
    };

    let mut hash = blake3::Hasher::new();
    serde_json::to_writer(&mut hash, &normalized)
        .map_err(|_| return ScreenFrameError::Serialization)?;

    return Ok(hash.finalize());
}

/// Compose a v2 state hash from metadata and cached row hashes in grid order.
///
/// Callers must supply one digest from [`hash_semantic_row`] per state grid row.
pub fn semantic_state_hash_from_row_hashes(
    state: &SemanticState,
    row_hashes: &[blake3::Hash],
) -> Result<String, ScreenFrameError> {
    let mut hash = semantic_state_metadata_hasher(state)?;
    for row_hash in row_hashes {
        hash.update(row_hash.as_bytes());
    }

    return Ok(hash.finalize().to_hex().to_string());
}

fn semantic_state_metadata_hasher(
    state: &SemanticState,
) -> Result<blake3::Hasher, ScreenFrameError> {
    let metadata = StateHashMetadata {
        cols: state.cols,
        rows: state.rows,
        cursor: &state.cursor,
        modes: &state.modes,
        colors: &state.colors,
        active_style: &state.active_style,
        metadata: &state.metadata,
    };

    let mut hash = blake3::Hasher::new();
    hash.update(SEMANTIC_STATE_HASH_DOMAIN);
    serde_json::to_writer(&mut hash, &metadata)
        .map_err(|_| return ScreenFrameError::Serialization)?;

    return Ok(hash);
}

/// Project viewport rows as text, replacing empty cells with spaces and
/// trimming only trailing ASCII spaces from each newline-separated row.
#[must_use]
pub fn semantic_viewport_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        for cell in &row.cells {
            if cell.grapheme.is_empty() {
                output.push(' ');
            } else {
                output.push_str(&cell.grapheme);
            }
        }

        while output.ends_with(' ') {
            output.pop();
        }

        if index + 1 < state.grid.len() {
            output.push('\n');
        }
    }

    return output;
}

/// Resolve and validate a requested region without allocating projection rows.
pub fn validate_screen_region(
    request: &ScreenProjectionRequest,
    cols: u16,
    rows: u16,
) -> Result<ScreenRegion, ScreenFrameError> {
    if request.detail == FrameDetail::Cells && request.region.is_none() {
        return Err(ScreenFrameError::InvalidRegion {
            region: None,
            cols,
            rows,
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
        });
    }

    let region = request.region.unwrap_or(ScreenRegion {
        top: 0,
        left: 0,
        right: cols,
        bottom: rows,
    });

    if region.right < region.left || region.bottom < region.top {
        return Err(invalid_region(
            Some(region),
            cols,
            rows,
            InvalidRegionReason::Reversed,
        ));
    }

    if region.right == region.left || region.bottom == region.top {
        return Err(invalid_region(
            Some(region),
            cols,
            rows,
            InvalidRegionReason::Empty,
        ));
    }

    if region.right > cols || region.bottom > rows {
        return Err(invalid_region(
            Some(region),
            cols,
            rows,
            InvalidRegionReason::OutOfBounds,
        ));
    }

    let cell_count = u32::from(region.width()) * u32::from(region.height());
    if cell_count > MAX_SCREEN_FRAME_CELLS {
        return Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::CellCount,
            requested: u64::from(cell_count),
            maximum: u64::from(MAX_SCREEN_FRAME_CELLS),
            detail: request.detail,
            region,
        });
    }
    return Ok(region);
}

/// Project one exact semantic state without I/O or mutation.
pub fn project_screen_frame(
    state: &SemanticState,
    request: &ScreenProjectionRequest,
    source: ScreenFrameSource,
    presented: bool,
    capture_quality: CaptureQuality,
) -> Result<ScreenFrame, ScreenFrameError> {
    validate_semantic_state(state)?;
    let region = validate_screen_region(request, state.cols, state.rows)?;
    validate_projection_clone_bytes(state, region, request.detail)?;

    let data = project_data(state, region, request.detail);
    let state_hash =
        semantic_state_hash(state).map_err(|_| return ScreenFrameError::Serialization)?;

    let mut frame = ScreenFrame {
        version: SCREEN_FRAME_VERSION,
        state_hash,
        presented,
        capture_quality,
        source,
        geometry: ScreenGeometry {
            cols: state.cols,
            rows: state.rows,
        },
        requested_region: region,
        returned_region: region,
        cursor: state.cursor.clone(),
        modes: state.modes.clone(),
        colors: state.colors.clone(),
        data,
        frame_hash: String::new(),
    };
    let (frame_hash, serialized_bytes) = screen_frame_hash_and_bytes(&frame)?;
    frame.frame_hash = frame_hash;

    if serialized_bytes > MAX_SCREEN_FRAME_JSON_BYTES {
        return Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested: serialized_bytes,
            maximum: MAX_SCREEN_FRAME_JSON_BYTES,
            detail: request.detail,
            region,
        });
    }
    return Ok(frame);
}

fn invalid_region(
    region: Option<ScreenRegion>,
    cols: u16,
    rows: u16,
    reason: InvalidRegionReason,
) -> ScreenFrameError {
    return ScreenFrameError::InvalidRegion {
        region,
        cols,
        rows,
        reason,
    };
}

fn validate_semantic_state(state: &SemanticState) -> Result<(), ScreenFrameError> {
    if state.grid.len() != usize::from(state.rows) {
        return Err(ScreenFrameError::InconsistentSemanticState {
            reason: SemanticStateFailure::RowCount,
        });
    }

    if state.colors.palette.len() != 256 {
        return Err(ScreenFrameError::InconsistentSemanticState {
            reason: SemanticStateFailure::PaletteSize,
        });
    }

    for (row_index, row) in state.grid.iter().enumerate() {
        if usize::from(row.row) != row_index {
            return Err(ScreenFrameError::InconsistentSemanticState {
                reason: SemanticStateFailure::RowCoordinate,
            });
        }

        if row.cells.len() != usize::from(state.cols) {
            return Err(ScreenFrameError::InconsistentSemanticState {
                reason: SemanticStateFailure::CellCount,
            });
        }

        if row
            .cells
            .iter()
            .enumerate()
            .any(|(column, cell)| return usize::from(cell.column) != column)
        {
            return Err(ScreenFrameError::InconsistentSemanticState {
                reason: SemanticStateFailure::CellCoordinate,
            });
        }
    }

    return Ok(());
}

fn validate_projection_clone_bytes(
    state: &SemanticState,
    region: ScreenRegion,
    detail: FrameDetail,
) -> Result<(), ScreenFrameError> {
    let mut requested = 0u64;
    for row in selected_rows(state, region) {
        for cell in selected_cells(row.cells.as_slice(), region) {
            let grapheme_bytes = json_string_content_bytes(&cell.grapheme)?;
            let convenience_bytes =
                if cell.grapheme.is_empty() && cell.wide_kind == WideKind::Narrow {
                    1
                } else {
                    grapheme_bytes
                };

            let cloned_bytes = match detail {
                FrameDetail::Text => convenience_bytes,
                FrameDetail::Spans => convenience_bytes
                    .saturating_mul(2)
                    .saturating_add(grapheme_bytes),
                FrameDetail::Cells => convenience_bytes.saturating_add(grapheme_bytes),
            };
            requested = requested.saturating_add(cloned_bytes);

            if requested > MAX_SCREEN_FRAME_JSON_BYTES {
                return Err(ScreenFrameError::ResourceLimit {
                    limit: ScreenFrameLimitKind::SerializedBytes,
                    requested,
                    maximum: MAX_SCREEN_FRAME_JSON_BYTES,
                    detail,
                    region,
                });
            }
        }
    }

    return Ok(());
}

fn json_string_content_bytes(value: &str) -> Result<u64, ScreenFrameError> {
    let mut count = 0u64;
    for byte in value.bytes() {
        let len = match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0C => 2,
            0x00..=0x1F => 6,
            _ => 1,
        };

        count = count
            .checked_add(len)
            .ok_or(ScreenFrameError::Serialization)?;
    }

    return Ok(count);
}

fn project_data(
    state: &SemanticState,
    region: ScreenRegion,
    detail: FrameDetail,
) -> ScreenFrameData {
    match detail {
        FrameDetail::Text => {
            let rows: Vec<ScreenTextRow> = selected_rows(state, region)
                .map(|row| {
                    return ScreenTextRow {
                        row: row.row,
                        soft_wrapped: row.soft_wrapped,
                        wrap_continuation: row.wrap_continuation,
                        text: convenience_text(selected_cells(row.cells.as_slice(), region)),
                    };
                })
                .collect();

            return ScreenFrameData::Text { rows };
        }
        FrameDetail::Spans => {
            let rows: Vec<ScreenSpanRow> = selected_rows(state, region)
                .map(|row| {
                    let cells = selected_cells(row.cells.as_slice(), region);

                    return ScreenSpanRow {
                        row: row.row,
                        soft_wrapped: row.soft_wrapped,
                        wrap_continuation: row.wrap_continuation,
                        text: convenience_text(cells),
                        spans: project_spans(cells),
                    };
                })
                .collect();

            return ScreenFrameData::Spans { rows };
        }
        FrameDetail::Cells => {
            let rows: Vec<ScreenCellRow> = selected_rows(state, region)
                .map(|row| {
                    let cells = selected_cells(row.cells.as_slice(), region);

                    return ScreenCellRow {
                        row: row.row,
                        soft_wrapped: row.soft_wrapped,
                        wrap_continuation: row.wrap_continuation,
                        text: convenience_text(cells),
                        cells: cells
                            .iter()
                            .map(|cell| return screen_cell(row.row, cell))
                            .collect(),
                    };
                })
                .collect();

            return ScreenFrameData::Cells { rows };
        }
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "validated by validate_screen_region"
)]
fn selected_rows(state: &SemanticState, region: ScreenRegion) -> impl Iterator<Item = &RowState> {
    let rows = &state.grid[usize::from(region.top)..usize::from(region.bottom)];

    return rows.iter();
}

#[expect(
    clippy::indexing_slicing,
    reason = "validated by validate_screen_region"
)]
fn selected_cells(cells: &[CellState], region: ScreenRegion) -> &[CellState] {
    return &cells[usize::from(region.left)..usize::from(region.right)];
}

fn screen_cell(row: u16, cell: &CellState) -> ScreenCell {
    return ScreenCell {
        row,
        column: cell.column,
        grapheme: cell.grapheme.clone(),
        wide_kind: cell.wide_kind,
        appearance: appearance(cell),
    };
}

fn appearance(cell: &CellState) -> ScreenAppearance {
    return ScreenAppearance {
        style: cell.style.clone(),
        resolved_foreground: cell.resolved_foreground,
        resolved_background: cell.resolved_background,
        selected: cell.selected,
    };
}

fn convenience_text(cells: &[CellState]) -> String {
    let mut text = String::new();
    for cell in cells {
        if !cell.grapheme.is_empty() {
            text.push_str(&cell.grapheme);
        } else if cell.wide_kind == WideKind::Narrow {
            text.push(' ');
        }
    }

    return text;
}

fn project_spans(cells: &[CellState]) -> Vec<ScreenSpan> {
    let mut spans: Vec<ScreenSpan> = Vec::new();
    for cell in cells {
        let cell_appearance = appearance(cell);

        if let Some(span) = spans
            .last_mut()
            .filter(|span| return span.appearance == cell_appearance)
        {
            span.end_column = cell.column.saturating_add(1);
            span.cell_count = span.cell_count.saturating_add(1);
            append_convenience_cell(&mut span.text, cell);
            span.cells.push(SpanCell {
                grapheme: cell.grapheme.clone(),
                wide_kind: cell.wide_kind,
            });
            continue;
        }

        let mut text = String::new();
        append_convenience_cell(&mut text, cell);
        spans.push(ScreenSpan {
            start_column: cell.column,
            end_column: cell.column.saturating_add(1),
            cell_count: 1,
            text,
            appearance: cell_appearance,
            cells: vec![SpanCell {
                grapheme: cell.grapheme.clone(),
                wide_kind: cell.wide_kind,
            }],
        });
    }

    return spans;
}

fn append_convenience_cell(text: &mut String, cell: &CellState) {
    if !cell.grapheme.is_empty() {
        text.push_str(&cell.grapheme);
    } else if cell.wide_kind == WideKind::Narrow {
        text.push(' ');
    }
}

#[derive(Serialize)]
struct ScreenFrameHashInput<'frame> {
    version: u32,
    state_hash: &'frame str,
    presented: bool,
    capture_quality: CaptureQuality,
    source: &'frame ScreenFrameSource,
    geometry: ScreenGeometry,
    requested_region: ScreenRegion,
    returned_region: ScreenRegion,
    cursor: &'frame CursorState,
    modes: &'frame ModeState,
    colors: &'frame ColorState,
    data: &'frame ScreenFrameData,
}

struct CountingHashWriter {
    count: u64,
    hasher: blake3::Hasher,
}

impl Write for CountingHashWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.count = self
            .count
            .checked_add(u64::try_from(buf.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| return io::Error::other("screen frame byte count overflow"))?;

        self.hasher.update(buf);

        return Ok(buf.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

/// Compute the canonical projection identity, excluding the identity field itself.
/// Compute canonical projection identity and total serialized frame byte length in one pass.
pub fn screen_frame_hash_and_bytes(frame: &ScreenFrame) -> Result<(String, u64), ScreenFrameError> {
    let input = ScreenFrameHashInput {
        version: frame.version,
        state_hash: &frame.state_hash,
        presented: frame.presented,
        capture_quality: frame.capture_quality,
        source: &frame.source,
        geometry: frame.geometry,
        requested_region: frame.requested_region,
        returned_region: frame.returned_region,
        cursor: &frame.cursor,
        modes: &frame.modes,
        colors: &frame.colors,
        data: &frame.data,
    };

    let mut writer = CountingHashWriter {
        count: 0,
        hasher: blake3::Hasher::new(),
    };
    writer.hasher.update(SCREEN_FRAME_HASH_DOMAIN);
    serde_json::to_writer(&mut writer, &input)
        .map_err(|_| return ScreenFrameError::Serialization)?;
    let hash = writer.hasher.finalize().to_hex().to_string();
    let total_bytes = writer.count.saturating_add(80);

    return Ok((hash, total_bytes));
}

/// Compute the canonical projection identity, excluding the identity field itself.
pub fn screen_frame_hash(frame: &ScreenFrame) -> Result<String, ScreenFrameError> {
    return screen_frame_hash_and_bytes(frame).map(|(hash, _)| return hash);
}

/// Pure deterministic text snapshot normalization.
pub fn normalize_screen_frame_text(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameData::Text { rows } = &frame.data else {
        return Err(ScreenFrameError::Serialization);
    };

    return normalized_value(frame, FrameDetail::Text, rows, options);
}

/// Pure deterministic span snapshot normalization.
pub fn normalize_screen_frame_spans(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameData::Spans { rows } = &frame.data else {
        return Err(ScreenFrameError::Serialization);
    };

    return normalized_value(frame, FrameDetail::Spans, rows, options);
}

/// Pure deterministic exact-cell snapshot normalization.
pub fn normalize_screen_frame_cells(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameData::Cells { rows } = &frame.data else {
        return Err(ScreenFrameError::Serialization);
    };

    return normalized_value(frame, FrameDetail::Cells, rows, options);
}

fn normalized_value<T: Serialize>(
    frame: &ScreenFrame,
    detail: FrameDetail,
    rows: &T,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let mut normalized = serde_json::Map::new();
    normalized.insert("version".to_owned(), serde_json::json!(frame.version));
    normalized.insert("geometry".to_owned(), serde_json::json!(frame.geometry));
    normalized.insert(
        "requested_region".to_owned(),
        serde_json::json!(frame.requested_region),
    );
    normalized.insert(
        "returned_region".to_owned(),
        serde_json::json!(frame.returned_region),
    );
    normalized.insert("detail".to_owned(), serde_json::json!(detail));
    normalized.insert("colors".to_owned(), serde_json::json!(frame.colors));
    normalized.insert(
        "rows".to_owned(),
        serde_json::to_value(rows).map_err(|_| return ScreenFrameError::Serialization)?,
    );

    if options.include_cursor {
        normalized.insert("cursor".to_owned(), serde_json::json!(frame.cursor));
    }

    if options.include_modes {
        normalized.insert("modes".to_owned(), serde_json::json!(frame.modes));
    }

    return Ok(serde_json::Value::Object(normalized));
}

#[cfg(test)]
mod tests;
