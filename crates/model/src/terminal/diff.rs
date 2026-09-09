//! Stable deterministic semantic endpoint comparison contracts.

use crate::{
    terminal::observation::StatePoint,
    terminal::state::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, ModeState, TerminalMetadata,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
