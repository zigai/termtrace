//! Bounded, versioned rich projections of authoritative semantic terminal state.

use std::{
    error::Error as StdError,
    fmt,
    io::{self, Write},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    control::ActorBoundaries,
    event_log::CanonicalBoundary,
    ghostty_vt::{
        CellState, CellStyle, ColorState, CursorState, ModeState, Rgb, RowState, SemanticState,
        WideKind,
    },
    observation::{
        CaptureQuality, ObservationIntegrity, ObservationResolution, ObservationSourceKind,
        StatePoint,
    },
    replay::semantic_state_hash,
};

/// Rich screen projection format version.
pub const SCREEN_FRAME_VERSION: u32 = 1;
/// Maximum terminal cells returned by one rich projection.
pub const MAX_SCREEN_FRAME_CELLS: u32 = 16_384;
/// Maximum serialized bytes in one rich projection.
pub const MAX_SCREEN_FRAME_JSON_BYTES: u64 = 768 * 1024;
const SCREEN_FRAME_HASH_DOMAIN: &[u8] = b"termtrace-screen-frame-v1\0";

/// Requested rich-screen representation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameDetail {
    /// Deterministic convenience text with authoritative row wrap facts.
    Text,
    /// Lossless appearance spans.
    #[default]
    Spans,
    /// Exact cells; requires an explicit region.
    Cells,
}

/// Absolute zero-based half-open terminal region.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenRegion {
    /// Inclusive top row.
    pub top: u16,
    /// Inclusive left column.
    pub left: u16,
    /// Exclusive right column.
    pub right: u16,
    /// Exclusive bottom row.
    pub bottom: u16,
}

impl ScreenRegion {
    /// Region width.
    #[must_use]
    pub fn width(self) -> u16 {
        return self.right.saturating_sub(self.left);
    }

    /// Region height.
    #[must_use]
    pub fn height(self) -> u16 {
        return self.bottom.saturating_sub(self.top);
    }
}

/// Options for one rich projection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenProjectionRequest {
    /// Requested representation.
    pub detail: FrameDetail,
    /// Exact absolute region, or the complete viewport for text/spans.
    pub region: Option<ScreenRegion>,
}

/// Complete terminal geometry, independent of the returned region.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenGeometry {
    /// Viewport columns.
    pub cols: u16,
    /// Viewport rows.
    pub rows: u16,
}

/// Exact provenance of the projected semantic state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenFrameSource {
    /// State selected while the live actor still owned exclusivity.
    Actor {
        /// Canonical session identity.
        session_id: Uuid,
        /// Durable and actor-observed canonical boundaries.
        boundaries: ActorBoundaries,
        /// Actor observation time in session-monotonic nanoseconds.
        observed_monotonic_ns: u64,
    },
    /// State selected through a completed or explicit semantic resolution.
    Resolved {
        /// Source category.
        source_kind: ObservationSourceKind,
        /// Canonical session identity when available.
        session_id: Option<Uuid>,
        /// Whether canonical evidence completed durably.
        source_completed: bool,
        /// Requested state selector.
        state_point: StatePoint,
        /// Dense semantic state identity when assigned.
        state_id: Option<u64>,
        /// Responsible canonical event sequence when known.
        event_seq: Option<u64>,
        /// Session-monotonic timestamp when known.
        monotonic_ns: Option<u64>,
        /// Exact validated source boundary.
        boundary: CanonicalBoundary,
        /// Integrity relationship to canonical evidence.
        integrity: ObservationIntegrity,
        /// Resolution path used to select the state.
        resolution: ObservationResolution,
    },
}

/// Complete appearance shared by an exact cell or a lossless span.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenAppearance {
    /// Authoritative source style and color selectors.
    pub style: CellStyle,
    /// Resolved explicit foreground before default fallback.
    pub resolved_foreground: Option<Rgb>,
    /// Resolved explicit background before default fallback.
    pub resolved_background: Option<Rgb>,
    /// Selection membership.
    pub selected: bool,
}

/// Exact projected terminal cell.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenCell {
    /// Absolute viewport row.
    pub row: u16,
    /// Absolute viewport column.
    pub column: u16,
    /// Complete grapheme cluster, empty for a blank/spacer.
    pub grapheme: String,
    /// Native Ghostty width role.
    pub wide_kind: WideKind,
    /// Complete semantic appearance.
    pub appearance: ScreenAppearance,
}

/// Per-column content retained inside an appearance span.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpanCell {
    /// Complete grapheme cluster.
    pub grapheme: String,
    /// Native Ghostty width role.
    pub wide_kind: WideKind,
}

/// Lossless run of cells with identical complete appearance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenSpan {
    /// Inclusive absolute start column.
    pub start_column: u16,
    /// Exclusive absolute end column.
    pub end_column: u16,
    /// Number of terminal columns covered.
    pub cell_count: u16,
    /// Deterministic convenience text for this run.
    pub text: String,
    /// Appearance common to every covered cell.
    pub appearance: ScreenAppearance,
    /// Exact grapheme/width map in column order.
    pub cells: Vec<SpanCell>,
}

/// Text-detail row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenTextRow {
    /// Absolute row.
    pub row: u16,
    /// Whether this row ends in a native soft wrap.
    pub soft_wrapped: bool,
    /// Whether this row continues a native soft wrap.
    pub wrap_continuation: bool,
    /// Deterministic convenience text.
    pub text: String,
}

/// Span-detail row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenSpanRow {
    /// Absolute row.
    pub row: u16,
    /// Whether this row ends in a native soft wrap.
    pub soft_wrapped: bool,
    /// Whether this row continues a native soft wrap.
    pub wrap_continuation: bool,
    /// Deterministic convenience text.
    pub text: String,
    /// Lossless ordered appearance spans.
    pub spans: Vec<ScreenSpan>,
}

/// Cell-detail row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenCellRow {
    /// Absolute row.
    pub row: u16,
    /// Whether this row ends in a native soft wrap.
    pub soft_wrapped: bool,
    /// Whether this row continues a native soft wrap.
    pub wrap_continuation: bool,
    /// Deterministic convenience text.
    pub text: String,
    /// Exact ordered cells.
    pub cells: Vec<ScreenCell>,
}

/// Structurally discriminated rich row payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "detail", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenFrameData {
    /// Convenience text and authoritative wrap facts.
    Text {
        /// Ordered projected rows.
        rows: Vec<ScreenTextRow>,
    },
    /// Lossless appearance spans.
    Spans {
        /// Ordered projected rows.
        rows: Vec<ScreenSpanRow>,
    },
    /// Exact cells.
    Cells {
        /// Ordered projected rows.
        rows: Vec<ScreenCellRow>,
    },
}

impl ScreenFrameData {
    /// Number of returned rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match self {
            Self::Text { rows } => return rows.len(),

            Self::Spans { rows } => return rows.len(),
            Self::Cells { rows } => return rows.len(),
        }
    }
}
/// Allocation-free row facts supplied while visiting projected cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenRowMetadata<'row> {
    /// Absolute row.
    pub row: u16,
    /// Native soft-wrap flag.
    pub soft_wrapped: bool,
    /// Native wrap-continuation flag.
    pub wrap_continuation: bool,
    /// Deterministic row convenience text.
    pub text: &'row str,
}

/// Versioned, bounded projection of one exact semantic state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenFrame {
    /// Projection format version.
    pub version: u32,
    /// Complete semantic v5 state identity.
    pub state_hash: String,
    /// Whether this state was presentation eligible.
    pub presented: bool,
    /// Settlement/provenance label.
    pub capture_quality: CaptureQuality,
    /// Exact source occurrence.
    pub source: ScreenFrameSource,
    /// Full viewport geometry.
    pub geometry: ScreenGeometry,
    /// Validated requested region.
    pub requested_region: ScreenRegion,
    /// Actually returned region; equal to requested in version 1.
    pub returned_region: ScreenRegion,
    /// Complete cursor state.
    pub cursor: CursorState,
    /// Complete relevant terminal modes.
    pub modes: ModeState,
    /// Current/default colors and complete palette.
    pub colors: ColorState,
    /// Structurally matched detail payload.
    pub data: ScreenFrameData,
    /// Projection identity in the `termtrace-screen-frame-v1` hash domain.
    pub frame_hash: String,
}
impl ScreenFrame {
    /// Visit every exact cell represented by span or cell detail without allocation.
    ///
    /// Text detail has no exact appearance data and therefore invokes no callbacks.
    pub fn for_each_cell(
        &self,
        mut visit: impl FnMut(ScreenRowMetadata<'_>, u16, &str, WideKind, &ScreenAppearance),
    ) {
        match &self.data {
            ScreenFrameData::Text { .. } => {}
            ScreenFrameData::Spans { rows } => {
                for row in rows {
                    let metadata = ScreenRowMetadata {
                        row: row.row,
                        soft_wrapped: row.soft_wrapped,
                        wrap_continuation: row.wrap_continuation,
                        text: &row.text,
                    };
                    for span in &row.spans {
                        for (offset, cell) in span.cells.iter().enumerate() {
                            let column = span
                                .start_column
                                .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
                            visit(
                                metadata,
                                column,
                                &cell.grapheme,
                                cell.wide_kind,
                                &span.appearance,
                            );
                        }
                    }
                }
            }
            ScreenFrameData::Cells { rows } => {
                for row in rows {
                    let metadata = ScreenRowMetadata {
                        row: row.row,
                        soft_wrapped: row.soft_wrapped,
                        wrap_continuation: row.wrap_continuation,
                        text: &row.text,
                    };
                    for cell in &row.cells {
                        visit(
                            metadata,
                            cell.column,
                            &cell.grapheme,
                            cell.wide_kind,
                            &cell.appearance,
                        );
                    }
                }
            }
        }
    }
}

/// Region validation failure reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvalidRegionReason {
    /// Exact cells were requested without an explicit region.
    CellsRequireExplicitRegion,
    /// Region has zero width or height.
    Empty,
    /// An edge is ordered before its origin.
    Reversed,
    /// Region lies outside the full viewport.
    OutOfBounds,
}

/// Resource controlled by a projection limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenFrameLimitKind {
    /// Returned terminal cells.
    CellCount,
    /// Serialized projection bytes.
    SerializedBytes,
    /// Complete enclosing response bytes.
    ResponseBytes,
}

/// Bounded semantic-state consistency failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticStateFailure {
    /// Grid row count differs from geometry.
    RowCount,
    /// A row's absolute coordinate differs from its position.
    RowCoordinate,
    /// A row's cell count differs from geometry.
    CellCount,
    /// A cell's absolute coordinate differs from its position.
    CellCoordinate,
    /// Color palette does not contain exactly 256 entries.
    PaletteSize,
}

/// Typed bounded rich-projection failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenFrameError {
    /// Region is malformed or outside the viewport.
    InvalidRegion {
        /// Requested region, when supplied.
        region: Option<ScreenRegion>,
        /// Full viewport columns.
        cols: u16,
        /// Full viewport rows.
        rows: u16,
        /// Stable invalidity category.
        reason: InvalidRegionReason,
    },
    /// Projection exceeded a centralized resource limit.
    ResourceLimit {
        /// Limit category that was exceeded.
        limit: ScreenFrameLimitKind,
        /// Requested or measured amount.
        requested: u64,
        /// Maximum allowed amount.
        maximum: u64,
        /// Requested projection detail.
        detail: FrameDetail,
        /// Validated requested region.
        region: ScreenRegion,
    },
    /// The semantic state violates canonical grid invariants.
    InconsistentSemanticState {
        /// Stable semantic-invariant category.
        reason: SemanticStateFailure,
    },
    /// Canonical serialization or hashing failed without including payload data.
    Serialization,
}

impl fmt::Display for ScreenFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(f, "screen frame projection failed: {self:?}");
    }
}

impl StdError for ScreenFrameError {}

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
    let cell_count = u32::from(region.width())
        .checked_mul(u32::from(region.height()))
        .ok_or_else(|| {
            return ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::CellCount,
                requested: u64::MAX,
                maximum: u64::from(MAX_SCREEN_FRAME_CELLS),
                detail: request.detail,
                region,
            };
        })?;
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
    frame.frame_hash = screen_frame_hash(&frame)?;
    let serialized_bytes = count_json_bytes(&frame)?;
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
    let mut writer = CountingWriter(0);
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| return ScreenFrameError::Serialization)?;
    return writer
        .0
        .checked_sub(2)
        .ok_or(ScreenFrameError::Serialization);
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

fn selected_rows(state: &SemanticState, region: ScreenRegion) -> impl Iterator<Item = &RowState> {
    let rows = state
        .grid
        .get(usize::from(region.top)..usize::from(region.bottom))
        .unwrap_or_default();
    return rows.iter();
}

fn selected_cells(cells: &[CellState], region: ScreenRegion) -> &[CellState] {
    return cells
        .get(usize::from(region.left)..usize::from(region.right))
        .unwrap_or_default();
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
pub fn screen_frame_hash(frame: &ScreenFrame) -> Result<String, ScreenFrameError> {
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
    return Ok(writer.hasher.finalize().to_hex().to_string());
}

struct CountingWriter(u64);

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(u64::try_from(buf.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| return io::Error::other("screen frame byte count overflow"))?;
        return Ok(buf.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

fn count_json_bytes(frame: &ScreenFrame) -> Result<u64, ScreenFrameError> {
    let mut writer = CountingWriter(0);
    serde_json::to_writer(&mut writer, frame)
        .map_err(|_| return ScreenFrameError::Serialization)?;
    return Ok(writer.0);
}

/// Stable snapshot fields optionally retained by pure normalizers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenSnapshotOptions {
    /// Retain cursor state.
    pub include_cursor: bool,
    /// Retain mode state.
    pub include_modes: bool,
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
mod tests {
    use super::*;
    use crate::ghostty_vt::GhosttyVt;
    use std::time::Instant;

    fn source(boundary: u64) -> ScreenFrameSource {
        let position = CanonicalBoundary {
            next_event_seq: boundary,
            event_hash: format!("hash-{boundary}"),
            output_bytes: boundary,
        };
        return ScreenFrameSource::Actor {
            session_id: Uuid::nil(),
            boundaries: ActorBoundaries {
                durable: position.clone(),
                observed: position,
            },
            observed_monotonic_ns: boundary,
        };
    }

    fn semantic_fixture() -> SemanticState {
        let mut terminal = GhosttyVt::new(6, 2, 0).expect("create terminal");
        terminal.write(b"\x1b[38;2;1;2;3mA");
        terminal.write("界".as_bytes());
        return terminal.snapshot().expect("semantic snapshot");
    }

    #[test]
    fn exact_cells_require_strict_explicit_region() {
        let missing = ScreenProjectionRequest {
            detail: FrameDetail::Cells,
            region: None,
        };
        assert!(matches!(
            validate_screen_region(&missing, 80, 24),
            Err(ScreenFrameError::InvalidRegion {
                reason: InvalidRegionReason::CellsRequireExplicitRegion,
                ..
            })
        ));
        let empty = ScreenProjectionRequest {
            detail: FrameDetail::Spans,
            region: Some(ScreenRegion {
                top: 0,
                left: 1,
                right: 1,
                bottom: 1,
            }),
        };
        assert!(matches!(
            validate_screen_region(&empty, 80, 24),
            Err(ScreenFrameError::InvalidRegion {
                reason: InvalidRegionReason::Empty,
                ..
            })
        ));
        let out_of_bounds = ScreenProjectionRequest {
            detail: FrameDetail::Spans,
            region: Some(ScreenRegion {
                top: 0,
                left: 0,
                right: 81,
                bottom: 24,
            }),
        };
        assert!(matches!(
            validate_screen_region(&out_of_bounds, 80, 24),
            Err(ScreenFrameError::InvalidRegion {
                reason: InvalidRegionReason::OutOfBounds,
                ..
            })
        ));
    }

    #[test]
    fn region_cell_limit_is_typed_and_never_clipped() {
        let request = ScreenProjectionRequest {
            detail: FrameDetail::Spans,
            region: Some(ScreenRegion {
                top: 0,
                left: 0,
                right: 200,
                bottom: 100,
            }),
        };
        assert_eq!(
            validate_screen_region(&request, 200, 100),
            Err(ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::CellCount,
                requested: 20_000,
                maximum: u64::from(MAX_SCREEN_FRAME_CELLS),
                detail: FrameDetail::Spans,
                region: request.region.expect("region"),
            })
        );
    }

    #[test]
    fn spans_expand_losslessly_and_keep_trailing_blank_cells() {
        let state = semantic_fixture();
        let request = ScreenProjectionRequest::default();
        let frame = project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live)
            .expect("project spans");
        let ScreenFrameData::Spans { rows } = &frame.data else {
            panic!("span detail");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0]
                .spans
                .iter()
                .map(|span| return usize::from(span.cell_count))
                .sum::<usize>(),
            6
        );
        assert_eq!(rows[0].spans.last().expect("trailing span").end_column, 6);
        let mut projected: Vec<(u16, String, WideKind)> = Vec::new();
        frame.for_each_cell(|_, column, grapheme, wide_kind, _| {
            projected.push((column, grapheme.to_owned(), wide_kind));
        });
        assert_eq!(projected.len(), 12);
        assert!(projected.iter().any(|(_, text, kind)| {
            return text == "界" && *kind == WideKind::Wide;
        }));
        assert!(projected.iter().any(|(_, _, kind)| {
            return *kind == WideKind::SpacerTail;
        }));
    }

    #[test]
    fn projection_hash_binds_detail_region_and_source_occurrence() {
        let state = semantic_fixture();
        let spans = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect("span frame");
        let cells = project_screen_frame(
            &state,
            &ScreenProjectionRequest {
                detail: FrameDetail::Cells,
                region: Some(ScreenRegion {
                    top: 0,
                    left: 0,
                    right: 3,
                    bottom: 1,
                }),
            },
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect("cell frame");
        let moved = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(2),
            true,
            CaptureQuality::Live,
        )
        .expect("different occurrence");
        assert_eq!(spans.state_hash, cells.state_hash);
        assert_ne!(spans.frame_hash, cells.frame_hash);
        assert_ne!(spans.frame_hash, moved.frame_hash);
    }

    #[test]
    fn snapshot_normalization_omits_volatile_source_identity() {
        let state = semantic_fixture();
        let first = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect("first");
        let second = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(2),
            true,
            CaptureQuality::Live,
        )
        .expect("second");
        assert_eq!(
            normalize_screen_frame_spans(&first, ScreenSnapshotOptions::default())
                .expect("normalize first"),
            normalize_screen_frame_spans(&second, ScreenSnapshotOptions::default())
                .expect("normalize second")
        );
    }

    #[test]
    fn serialized_byte_limit_returns_bounded_error() {
        let mut state = semantic_fixture();
        state.grid[0].cells[0].grapheme =
            "x".repeat(usize::try_from(MAX_SCREEN_FRAME_JSON_BYTES).expect("byte limit") + 1);
        let error = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect_err("oversized projection");
        assert!(matches!(
            error,
            ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::SerializedBytes,
                requested,
                maximum: MAX_SCREEN_FRAME_JSON_BYTES,
                detail: FrameDetail::Spans,
                ..
            } if requested > MAX_SCREEN_FRAME_JSON_BYTES
        ));
    }

    #[test]
    fn escaped_projection_strings_are_bounded_before_projection_clones() {
        let mut state = semantic_fixture();
        state.grid[0].cells[0].grapheme = "\u{0001}".repeat(50_000);
        let region =
            validate_screen_region(&ScreenProjectionRequest::default(), state.cols, state.rows)
                .expect("region");
        assert!(matches!(
            validate_projection_clone_bytes(&state, region, FrameDetail::Spans),
            Err(ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::SerializedBytes,
                requested,
                maximum: MAX_SCREEN_FRAME_JSON_BYTES,
                ..
            }) if requested > MAX_SCREEN_FRAME_JSON_BYTES
        ));
    }

    #[test]
    fn span_boundaries_preserve_complete_selection_appearance() {
        let mut state = semantic_fixture();
        state.grid[0].cells[1].selected = true;
        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect("span frame");
        let ScreenFrameData::Spans { rows } = frame.data else {
            panic!("span detail");
        };
        assert!(rows[0].spans.iter().any(|span| {
            return span.start_column == 1 && span.end_column == 2 && span.appearance.selected;
        }));
    }
    #[test]
    #[ignore = "manual rich-frame payload and projection benchmark"]
    fn common_viewport_projection_metrics() {
        for (cols, rows) in [(80, 24), (160, 45)] {
            let mut terminal = GhosttyVt::new(cols, rows, 0).expect("create benchmark terminal");
            let state = terminal.snapshot().expect("benchmark semantic snapshot");
            for detail in [FrameDetail::Spans, FrameDetail::Cells] {
                let request = ScreenProjectionRequest {
                    detail,
                    region: (detail == FrameDetail::Cells).then_some(ScreenRegion {
                        top: 0,
                        left: 0,
                        right: cols,
                        bottom: rows,
                    }),
                };
                let started = Instant::now();
                let result =
                    project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live);
                let elapsed = started.elapsed();
                match result {
                    Ok(frame) => {
                        let bytes = serde_json::to_vec(&frame)
                            .expect("serialize benchmark projection")
                            .len();
                        assert!(
                            u64::try_from(bytes).unwrap_or(u64::MAX) <= MAX_SCREEN_FRAME_JSON_BYTES
                        );
                        println!(
                            "screen_frame detail={detail:?} cols={cols} rows={rows} \
                             status=ok payload_bytes={bytes} projection_us={}",
                            elapsed.as_micros()
                        );
                    }
                    Err(ScreenFrameError::ResourceLimit {
                        limit: ScreenFrameLimitKind::SerializedBytes,
                        requested,
                        maximum,
                        ..
                    }) => {
                        assert!(requested > maximum);
                        println!(
                            "screen_frame detail={detail:?} cols={cols} rows={rows} \
                             status=serialized_limit payload_bytes={requested} \
                             maximum={maximum} projection_us={}",
                            elapsed.as_micros()
                        );
                    }
                    Err(error) => {
                        panic!("unexpected benchmark projection error: {error}");
                    }
                }
            }
        }
    }
}
