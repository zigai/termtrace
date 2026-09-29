//! Bounded, versioned rich projections of authoritative semantic terminal state.

use crate::{
    execution::event_log::CanonicalBoundary,
    terminal::observation::ActorBoundaries,
    terminal::observation::{
        CaptureQuality, ObservationIntegrity, ObservationResolution, ObservationSourceKind,
        StatePoint,
    },
    terminal::state::{CellStyle, ColorState, CursorState, ModeState, Rgb, WideKind},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt};
use uuid::Uuid;

/// Rich screen projection format version.
pub const SCREEN_FRAME_VERSION: u32 = 1;
/// Absolute runaway safety cap for malicious input (supporting dual-4K/ultrawide viewports).
pub const CANVAS_MAX_CELLS: u32 = 262_144;
/// Default maximum serialized bytes in one rich projection.
pub const DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum terminal cells returned by one rich projection.
pub const MAX_SCREEN_FRAME_CELLS: u32 = CANVAS_MAX_CELLS;
/// Maximum serialized bytes in one rich projection.
pub const MAX_SCREEN_FRAME_JSON_BYTES: u64 = DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES;

/// Stable snapshot fields optionally retained by pure normalizers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenSnapshotOptions {
    /// Retain cursor state.
    pub include_cursor: bool,
    /// Retain mode state.
    pub include_modes: bool,
}

/// Requested rich-screen representation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameDetail {
    /// Observation metadata only; no cell or text payload.
    None,
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
    /// Observation metadata only; no cell or text payload.
    None,
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
            ScreenFrameData::None | ScreenFrameData::Text { .. } => {}
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

impl ScreenFrameError {
    /// Collapse a serializer failure into the payload-free `Serialization` category.
    ///
    /// This error is itself a serialized protocol value that must not carry payload data, so
    /// the serializer's detail is deliberately dropped at this boundary.
    pub fn serialization(_: serde_json::Error) -> Self {
        return Self::Serialization;
    }
}

impl fmt::Display for ScreenFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(f, "screen frame projection failed: {self:?}");
    }
}

impl StdError for ScreenFrameError {}
