use std::path::PathBuf;

use termtrace_model::{terminal::observation::ObservationFrame, terminal::state::SemanticState};

use crate::{Error, Result};

pub(super) fn frame_from_state(state: &SemanticState) -> ObservationFrame {
    return ObservationFrame {
        viewport_text: state_text(state),
        cols: state.cols,
        rows: state.rows,
        cursor_column: state.cursor.column,
        cursor_row: state.cursor.row,
        cursor_visible: state.cursor.visible,
        modes: state.modes.clone(),
    };
}

/// Render a semantic terminal grid as trimmed, newline-separated viewport text.
pub fn state_text(state: &SemanticState) -> String {
    return termtrace_terminal::semantic_viewport_text(state);
}

pub(super) fn frame_hash(frame: &ObservationFrame) -> Result<String> {
    let bytes = serde_json::to_vec(frame).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<observation-frame>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

pub(super) fn observation_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "observe terminal state",
        reason: reason.into(),
    };
}
