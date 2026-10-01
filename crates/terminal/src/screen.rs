//! Bounded rich projections of authoritative semantic terminal state.

use serde::Serialize;
use std::io::{self, Write};
pub use termtrace_model::terminal::screen::*;
use termtrace_model::{
    terminal::observation::CaptureQuality,
    terminal::state::{
        CellState, ColorSpec, ColorState, CursorState, CursorStyle, ModeState, RowState,
        SemanticState, WideKind,
    },
};

const SCREEN_FRAME_HASH_DOMAIN: &[u8] = b"termtrace-screen-frame-v1\0";
const SEMANTIC_STATE_HASH_DOMAIN: &[u8] = b"termtrace-semantic-state-v3\0";
const SEMANTIC_ROW_HASH_DOMAIN: &[u8] = b"termtrace-semantic-row-v3\0";

/// Hash normalized semantic terminal state using the stable v2 contract.
///
/// # Errors
///
/// Currently infallible; the `ScreenFrameError` result is part of the stable hashing signature.
pub fn semantic_state_hash(state: &SemanticState) -> Result<String, ScreenFrameError> {
    let mut hash = semantic_state_metadata_hasher(state);
    for row in &state.grid {
        hash.update(hash_semantic_row(row)?.as_bytes());
    }

    return Ok(hash.finalize().to_hex().to_string());
}

fn hash_color_spec(hasher: &mut blake3::Hasher, spec: ColorSpec) {
    match spec {
        ColorSpec::None => hasher.update(&[0, 0, 0, 0]),
        ColorSpec::Palette(idx) => hasher.update(&[1, idx, 0, 0]),
        ColorSpec::Rgb(rgb) => hasher.update(&[2, rgb.r, rgb.g, rgb.b]),
    };
}

fn hash_cell(hasher: &mut blake3::Hasher, cell: &CellState) {
    hasher.update(&cell.column.to_le_bytes());
    let wide_byte = match cell.wide_kind {
        WideKind::Narrow => 0,
        WideKind::Wide => 1,
        WideKind::SpacerTail => 2,
        WideKind::SpacerHead => 3,
    };
    hasher.update(&[wide_byte, u8::from(cell.selected)]);

    match cell.resolved_foreground {
        Some(rgb) => hasher.update(&[1, rgb.r, rgb.g, rgb.b]),
        None => hasher.update(&[0, 0, 0, 0]),
    };

    match cell.resolved_background {
        Some(rgb) => hasher.update(&[1, rgb.r, rgb.g, rgb.b]),
        None => hasher.update(&[0, 0, 0, 0]),
    };

    let mut bits = 0_u16;
    if cell.style.bold {
        bits |= 1 << 0;
    }

    if cell.style.italic {
        bits |= 1 << 1;
    }

    if cell.style.faint {
        bits |= 1 << 2;
    }

    if cell.style.blink {
        bits |= 1 << 3;
    }

    if cell.style.inverse {
        bits |= 1 << 4;
    }

    if cell.style.invisible {
        bits |= 1 << 5;
    }

    if cell.style.strikethrough {
        bits |= 1 << 6;
    }

    if cell.style.overline {
        bits |= 1 << 7;
    }

    hasher.update(&bits.to_le_bytes());
    hasher.update(&cell.style.underline.to_le_bytes());
    hash_color_spec(hasher, cell.style.foreground);
    hash_color_spec(hasher, cell.style.background);
    hash_color_spec(hasher, cell.style.underline_color);

    let bytes = cell.grapheme.as_bytes();
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    hasher.update(&len.to_le_bytes());
    hasher.update(bytes);
}

/// Hash one semantic row, excluding transient renderer dirtiness.
///
/// # Errors
///
/// Currently infallible; the `ScreenFrameError` result is part of the stable hashing signature.
pub fn hash_semantic_row(row: &RowState) -> Result<blake3::Hash, ScreenFrameError> {
    let mut hash = blake3::Hasher::new();
    hash.update(SEMANTIC_ROW_HASH_DOMAIN);
    hash.update(&row.row.to_le_bytes());
    hash.update(&[u8::from(row.soft_wrapped), u8::from(row.wrap_continuation)]);
    let cell_count = u32::try_from(row.cells.len()).unwrap_or(u32::MAX);
    hash.update(&cell_count.to_le_bytes());

    for cell in &row.cells {
        hash_cell(&mut hash, cell);
    }

    return Ok(hash.finalize());
}

/// Compose a v3 state hash from metadata and cached row hashes in grid order.
///
/// Callers must supply one digest from [`hash_semantic_row`] per state grid row.
///
/// # Errors
///
/// Currently infallible; the `ScreenFrameError` result is part of the stable hashing signature.
pub fn semantic_state_hash_from_row_hashes(
    state: &SemanticState,
    row_hashes: &[blake3::Hash],
) -> Result<String, ScreenFrameError> {
    let mut hash = semantic_state_metadata_hasher(state);
    for row_hash in row_hashes {
        hash.update(row_hash.as_bytes());
    }

    return Ok(hash.finalize().to_hex().to_string());
}

fn semantic_state_metadata_hasher(state: &SemanticState) -> blake3::Hasher {
    let mut hash = blake3::Hasher::new();
    hash.update(SEMANTIC_STATE_HASH_DOMAIN);
    hash.update(&state.cols.to_le_bytes());
    hash.update(&state.rows.to_le_bytes());

    // Cursor
    hash.update(&[
        u8::from(state.cursor.visible),
        u8::from(state.cursor.blinking),
        u8::from(state.cursor.in_viewport),
    ]);
    hash.update(&state.cursor.column.unwrap_or(u16::MAX).to_le_bytes());
    hash.update(&state.cursor.row.unwrap_or(u16::MAX).to_le_bytes());
    hash.update(&[state.cursor.wide_tail.map_or(2, |v| return u8::from(v))]);
    let style_id = match state.cursor.style {
        CursorStyle::Bar => 0,
        CursorStyle::Block => 1,
        CursorStyle::Underline => 2,
        CursorStyle::HollowBlock => 3,
    };
    hash.update(&[style_id]);

    // Modes
    hash.update(&[
        u8::from(state.modes.synchronized_output),
        u8::from(state.modes.alternate_screen),
        u8::from(state.modes.wraparound),
        u8::from(state.modes.bracketed_paste),
    ]);
    let mode_count = u32::try_from(state.modes.values.len()).unwrap_or(u32::MAX);
    hash.update(&mode_count.to_le_bytes());

    for mode in &state.modes.values {
        hash.update(&mode.number.to_le_bytes());
        hash.update(&[u8::from(mode.enabled)]);
    }

    // Colors
    hash.update(&[
        state.colors.background.r,
        state.colors.background.g,
        state.colors.background.b,
    ]);
    hash.update(&[
        state.colors.foreground.r,
        state.colors.foreground.g,
        state.colors.foreground.b,
    ]);

    if let Some(cursor_col) = state.colors.cursor {
        hash.update(&[1, cursor_col.r, cursor_col.g, cursor_col.b]);
    } else {
        hash.update(&[0, 0, 0, 0]);
    }

    let palette_count = u32::try_from(state.colors.palette.len()).unwrap_or(u32::MAX);
    hash.update(&palette_count.to_le_bytes());

    for col in &state.colors.palette {
        hash.update(&[col.r, col.g, col.b]);
    }

    // Active style
    let mut active_bits = 0_u16;
    if state.active_style.bold {
        active_bits |= 1 << 0;
    }

    if state.active_style.italic {
        active_bits |= 1 << 1;
    }

    if state.active_style.faint {
        active_bits |= 1 << 2;
    }

    if state.active_style.blink {
        active_bits |= 1 << 3;
    }

    if state.active_style.inverse {
        active_bits |= 1 << 4;
    }

    if state.active_style.invisible {
        active_bits |= 1 << 5;
    }

    if state.active_style.strikethrough {
        active_bits |= 1 << 6;
    }

    if state.active_style.overline {
        active_bits |= 1 << 7;
    }

    hash.update(&active_bits.to_le_bytes());
    hash.update(&state.active_style.underline.to_le_bytes());
    hash_color_spec(&mut hash, state.active_style.foreground);
    hash_color_spec(&mut hash, state.active_style.background);
    hash_color_spec(&mut hash, state.active_style.underline_color);

    // Metadata
    let title_bytes = state.metadata.title.as_bytes();
    let title_len = u32::try_from(title_bytes.len()).unwrap_or(u32::MAX);
    hash.update(&title_len.to_le_bytes());
    hash.update(title_bytes);

    let pwd_bytes = state.metadata.working_directory.as_bytes();
    let pwd_len = u32::try_from(pwd_bytes.len()).unwrap_or(u32::MAX);
    hash.update(&pwd_len.to_le_bytes());
    hash.update(pwd_bytes);

    return hash;
}

/// Project viewport rows as text, replacing empty cells with spaces and
/// trimming only trailing ASCII spaces from each newline-separated row.
#[must_use]
pub fn semantic_viewport_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        for cell in &row.cells {
            if !cell.grapheme.is_empty() {
                output.push_str(&cell.grapheme);
            } else if cell.wide_kind == WideKind::Narrow {
                output.push(' ');
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
///
/// # Errors
///
/// Returns `ScreenFrameError::InvalidRegion` when a cell projection has no explicit region or the
/// region is reversed, empty, or outside the `cols` by `rows` viewport, and
/// `ScreenFrameError::ResourceLimit` when it covers more cells than a projection allows.
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
    if cell_count > CANVAS_MAX_CELLS {
        return Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::CellCount,
            requested: u64::from(cell_count),
            maximum: u64::from(CANVAS_MAX_CELLS),
            detail: request.detail,
            region,
        });
    }
    return Ok(region);
}

/// Project one exact semantic state without I/O or mutation.
///
/// # Errors
///
/// Returns `ScreenFrameError::InconsistentSemanticState` when `state` violates grid invariants, the
/// region errors of [`validate_screen_region`], `ScreenFrameError::ResourceLimit` when the
/// projection would exceed its byte budget, and `ScreenFrameError::Serialization` when the frame
/// cannot be hashed.
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
    let state_hash = semantic_state_hash(state)?;

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

    let cell_count = u32::from(region.width()) * u32::from(region.height());
    let max_bytes = max_projection_bytes(cell_count);

    if serialized_bytes > max_bytes {
        return Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested: serialized_bytes,
            maximum: max_bytes,
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

const BYTES_PER_CELL_ESTIMATE: u64 = 128;

/// Scale allowed serialization bytes with the requested region size.
#[must_use]
pub fn max_projection_bytes(cell_count: u32) -> u64 {
    return DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES
        .max(u64::from(cell_count).saturating_mul(BYTES_PER_CELL_ESTIMATE));
}

fn validate_projection_clone_bytes(
    state: &SemanticState,
    region: ScreenRegion,
    detail: FrameDetail,
) -> Result<(), ScreenFrameError> {
    if detail == FrameDetail::None {
        return Ok(());
    }

    let cell_count = u32::from(region.width()) * u32::from(region.height());
    let max_bytes = max_projection_bytes(cell_count);

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
                FrameDetail::None => 0,
                FrameDetail::Text => convenience_bytes,
                FrameDetail::Spans => convenience_bytes
                    .saturating_mul(2)
                    .saturating_add(grapheme_bytes),
                FrameDetail::Cells => convenience_bytes.saturating_add(grapheme_bytes),
            };
            requested = requested.saturating_add(cloned_bytes);

            if requested > max_bytes {
                return Err(ScreenFrameError::ResourceLimit {
                    limit: ScreenFrameLimitKind::SerializedBytes,
                    requested,
                    maximum: max_bytes,
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
) -> ScreenFrameContent {
    match detail {
        FrameDetail::None => return ScreenFrameContent::None,
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

            return ScreenFrameContent::Text { rows };
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

            return ScreenFrameContent::Spans { rows };
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

            return ScreenFrameContent::Cells { rows };
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
        grapheme: cell.grapheme.to_string(),
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
                grapheme: cell.grapheme.to_string(),
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
                grapheme: cell.grapheme.to_string(),
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
    data: &'frame ScreenFrameContent,
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
///
/// # Errors
///
/// Returns `ScreenFrameError::Serialization` when the frame cannot be serialized.
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
    serde_json::to_writer(&mut writer, &input).map_err(ScreenFrameError::serialization)?;
    let hash = writer.hasher.finalize().to_hex().to_string();
    let total_bytes = writer.count.saturating_add(80);

    return Ok((hash, total_bytes));
}

/// Compute the canonical projection identity, excluding the identity field itself.
///
/// # Errors
///
/// Returns `ScreenFrameError::Serialization` when the frame cannot be serialized.
pub fn screen_frame_hash(frame: &ScreenFrame) -> Result<String, ScreenFrameError> {
    return screen_frame_hash_and_bytes(frame).map(|(hash, _)| return hash);
}

/// Pure deterministic text snapshot normalization.
///
/// # Errors
///
/// Returns `ScreenFrameError::Serialization` when `frame` does not hold text rows or cannot be
/// serialized.
pub fn normalize_screen_frame_text(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameContent::Text { rows } = &frame.data else {
        return Err(ScreenFrameError::Serialization);
    };

    return normalized_value(frame, FrameDetail::Text, rows, options);
}

/// Pure deterministic span snapshot normalization.
///
/// # Errors
///
/// Returns `ScreenFrameError::Serialization` when `frame` does not hold span rows or cannot be
/// serialized.
pub fn normalize_screen_frame_spans(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameContent::Spans { rows } = &frame.data else {
        return Err(ScreenFrameError::Serialization);
    };

    return normalized_value(frame, FrameDetail::Spans, rows, options);
}

/// Pure deterministic exact-cell snapshot normalization.
///
/// # Errors
///
/// Returns `ScreenFrameError::Serialization` when `frame` does not hold cell rows or cannot be
/// serialized.
pub fn normalize_screen_frame_cells(
    frame: &ScreenFrame,
    options: ScreenSnapshotOptions,
) -> Result<serde_json::Value, ScreenFrameError> {
    let ScreenFrameContent::Cells { rows } = &frame.data else {
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
        serde_json::to_value(rows).map_err(ScreenFrameError::serialization)?,
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
