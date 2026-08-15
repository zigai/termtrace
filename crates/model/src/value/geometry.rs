//! Terminal geometry measurement and playback verification.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Geometry reported by a live terminal through XTWINOPS queries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalGeometryReport {
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Width of one rendered cell in pixels.
    pub cell_width: u16,
    /// Height of one rendered cell in pixels.
    pub cell_height: u16,
    /// Width of the complete text area in pixels.
    pub text_width: u16,
    /// Height of the complete text area in pixels.
    pub text_height: u16,
}
