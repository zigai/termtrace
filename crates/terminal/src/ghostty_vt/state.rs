//! Stable model types and adapter-local formatting controls.

pub(super) use termtrace_model::terminal::state::{
    CellState, CellStyle, ColorSpec, ColorState, CursorState, CursorStyle, DirtyState, EncodedKey,
    FocusEvent, KeyModifiers, ModeState, MouseAction, MouseButton, NamedKey, Rgb, RowState,
    SemanticState, TerminalMetadata, TerminalMode, WideKind, MAX_TERMINAL_CELLS,
    MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS,
};

/// Return the cell count when a terminal geometry satisfies the shared policy.
#[must_use]
pub fn supported_geometry_cell_count(cols: u16, rows: u16) -> Option<u32> {
    let cells = u32::from(cols).checked_mul(u32::from(rows))?;

    return (cols > 0
        && rows > 0
        && cols <= MAX_TERMINAL_COLUMNS
        && rows <= MAX_TERMINAL_ROWS
        && cells <= MAX_TERMINAL_CELLS)
        .then_some(cells);
}

/// Formatter outputs supported by libghostty-vt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    /// Plain UTF-8 text.
    Plain,
    /// VT stream that reconstructs styles and terminal state.
    Vt,
    /// Self-contained HTML fragment.
    Html,
}
