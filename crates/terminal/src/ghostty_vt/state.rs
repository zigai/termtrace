//! Stable model types and adapter-local formatting controls.

pub(super) use termtrace_model::terminal::state::supported_geometry_cell_count;
pub(super) use termtrace_model::terminal::state::{
    CellState, CellStyle, ColorSpec, ColorState, CursorState, CursorStyle, DirtyState, EncodedKey,
    FocusEvent, KeyModifiers, ModeState, MouseAction, MouseButton, NamedKey, Rgb, RowState,
    SemanticState, TerminalMetadata, TerminalMode, WideKind,
};

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
