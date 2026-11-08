//! Bounded, versioned rich projections of authoritative semantic terminal state.

use crate::{
    execution::event_log::CanonicalBoundary,
    terminal::observation::ActorBoundaries,
    terminal::observation::{
        CaptureQuality, ObservationIntegrity, ObservationResolution, ObservationSourceKind,
        StatePoint,
    },
    terminal::state::{
        CellStyle, ColorState, CursorState, ModeState, Rgb, SemanticState, WideKind,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt};
use uuid::Uuid;

/// Text for screen matching with soft wraps joined and byte offsets mapped to viewport rows.
pub fn screen_match_text(state: &SemanticState) -> (String, Vec<(usize, u16)>) {
    let mut text = String::new();
    let mut starts = Vec::with_capacity(state.grid.len());

    for row in &state.grid {
        let start = text.len();
        starts.push((start, row.row));

        for cell in row.cells.iter() {
            if !cell.grapheme.is_empty() {
                text.push_str(&cell.grapheme);
            } else if cell.wide_kind == WideKind::Narrow {
                text.push(' ');
            }
        }

        if !row.soft_wrapped {
            while text.len() > start && text.ends_with(' ') {
                text.pop();
            }

            text.push('\n');
        }
    }

    return (text, starts);
}

/// Rich screen projection format version.
pub const SCREEN_FRAME_VERSION: u32 = 2;
/// Absolute runaway safety cap for malicious input (supporting dual-4K/ultrawide viewports).
pub const CANVAS_MAX_CELLS: u32 = 262_144;
/// Default maximum serialized bytes in one rich projection.
pub const DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum terminal cells returned by one rich projection.
pub const MAX_SCREEN_FRAME_CELLS: u32 = CANVAS_MAX_CELLS;
/// Maximum serialized bytes in one rich projection.
pub const MAX_SCREEN_FRAME_JSON_BYTES: u64 = DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES;
/// Unsigned 64-bit field names in screen frames, observations, state points, screen
/// predicates, stable captures, and screen waits.
///
/// SDK bindings carry exactly these fields as host big integers (Python `int`, TypeScript
/// `bigint`) and every other number as an ordinary host number.
pub const SDK_WIDE_UNSIGNED_FIELDS: &[&str] = &[
    "event_seq",
    "first_match_monotonic_ns",
    "first_observed_monotonic_ns",
    "minimum",
    "monotonic_ns",
    "next_event_seq",
    "observed_monotonic_ns",
    "output_bytes",
    "stable_for_ns",
    "state_id",
];
/// Raw byte field names in the same observation types, serialized as standard padded Base64
/// and carried by SDK bindings as host byte strings.
pub const SDK_BYTE_FIELDS: &[&str] = &["base64", "semantic_state_zstd"];

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
        /// Stream position of the presented state, as replay of the session numbers it.
        state_id: u64,
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
        /// Stream position of the state when known.
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
#[schemars(rename = "ScreenFrameData")]
pub enum ScreenFrameContent {
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
    pub data: ScreenFrameContent,
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
            ScreenFrameContent::None | ScreenFrameContent::Text { .. } => {}
            ScreenFrameContent::Spans { rows } => {
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
            ScreenFrameContent::Cells { rows } => {
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

#[cfg(test)]
mod tests {
    use super::{ScreenFrame, SDK_BYTE_FIELDS, SDK_WIDE_UNSIGNED_FIELDS};
    use crate::{
        protocol::control::ScreenPredicate,
        terminal::observation::{Observation, StatePoint},
    };
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn resolve<'schema>(root: &'schema Value, schema: &'schema Value) -> Vec<&'schema Value> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let pointer = reference.trim_start_matches('#');

            return root
                .pointer(pointer)
                .map(|target| return resolve(root, target))
                .unwrap_or_default();
        }

        let mut resolved = vec![schema];
        for combinator in ["anyOf", "oneOf", "allOf"] {
            for member in schema
                .get(combinator)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                resolved.extend(resolve(root, member));
            }
        }

        if let Some(items) = schema.get("items") {
            resolved.extend(resolve(root, items));
        }

        return resolved;
    }

    fn collect(
        root: &Value,
        schema: &Value,
        wide: &mut BTreeSet<String>,
        bytes: &mut BTreeSet<String>,
    ) {
        match schema {
            Value::Object(map) => {
                if let Some(Value::Object(properties)) = map.get("properties") {
                    for (name, property) in properties {
                        for resolved in resolve(root, property) {
                            let format = resolved.get("format").and_then(Value::as_str);
                            if matches!(format, Some("uint64" | "int64" | "uint" | "int")) {
                                wide.insert(name.clone());
                            }

                            if resolved.get("contentEncoding").and_then(Value::as_str)
                                == Some("base64")
                            {
                                bytes.insert(name.clone());
                            }
                        }
                    }
                }

                for value in map.values() {
                    collect(root, value, wide, bytes);
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(root, value, wide, bytes);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn sdk_field_tables_cover_every_wide_and_byte_schema_field() {
        let mut wide = BTreeSet::new();
        let mut bytes = BTreeSet::new();

        for schema in [
            schemars::schema_for!(ScreenFrame),
            schemars::schema_for!(Observation),
            schemars::schema_for!(StatePoint),
            schemars::schema_for!(ScreenPredicate),
        ] {
            let root = serde_json::to_value(&schema).expect("schema JSON");
            collect(&root, &root, &mut wide, &mut bytes);
        }

        // Stable-capture and screen-wait outcomes are service types without a schema; their
        // timestamp and byte fields are listed explicitly.
        wide.extend(
            [
                "first_match_monotonic_ns",
                "first_observed_monotonic_ns",
                "observed_monotonic_ns",
                "stable_for_ns",
            ]
            .map(str::to_owned),
        );

        bytes.insert("semantic_state_zstd".to_owned());

        let expected_wide: BTreeSet<String> = SDK_WIDE_UNSIGNED_FIELDS
            .iter()
            .map(|name| return (*name).to_owned())
            .collect();

        let expected_bytes: BTreeSet<String> = SDK_BYTE_FIELDS
            .iter()
            .map(|name| return (*name).to_owned())
            .collect();

        assert_eq!(wide, expected_wide);
        assert_eq!(bytes, expected_bytes);
    }
}
