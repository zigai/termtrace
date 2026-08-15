//! Deterministic PNG and opaque RGBA rasterization.

use std::sync::Arc;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use termtrace_model::terminal::state::{MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS, SemanticState};
use termtrace_terminal::supported_geometry_cell_count;

use crate::error::{Result, render_error};
use crate::font::semantic_font_database;
use crate::semantic_svg::render_svg_scene;

/// Stable semantic PNG rasterizer identity.
pub const SEMANTIC_PNG_RENDERER: &str = "termtrace-semantic-png-resvg-v2";

/// Rasterize the exact deterministic SVG scene with only the bundled font.
///
/// # Errors
///
/// Returns an error when the semantic geometry is outside the supported bounds,
/// the SVG cannot be parsed, or the PNG cannot be encoded.
pub fn render_png(state: &SemanticState) -> Result<Vec<u8>> {
    validate_geometry(state.cols, state.rows, "semantic state")?;
    let svg = render_svg_scene(state, state.cols, state.rows, false);
    return rasterize_svg(&svg);
}

/// Render one semantic state onto a fixed top-left-anchored video canvas.
///
/// The returned bytes are an opaque RGBA frame. The canvas may be larger than
/// the state's current grid so recordings containing PTY resizes can retain a
/// stable encoded-video geometry.
///
/// # Errors
///
/// Returns an error when the canvas is smaller than the semantic viewport,
/// either geometry is outside the supported bounds, or the SVG scene cannot be
/// rasterized.
pub fn render_video_rgba(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
) -> Result<Vec<u8>> {
    validate_geometry(state.cols, state.rows, "semantic state")?;
    validate_canvas_geometry(canvas_cols, canvas_rows)?;
    if canvas_cols < state.cols || canvas_rows < state.rows {
        return Err(render_error(format!(
            "semantic video canvas {canvas_cols}x{canvas_rows} is smaller than state {}x{}",
            state.cols, state.rows
        )));
    }
    let pixmap = rasterize_svg_pixmap(&render_svg_scene(state, canvas_cols, canvas_rows, false))?;
    return Ok(pixmap.data().to_vec());
}

pub(super) fn rasterize_svg(svg: &[u8]) -> Result<Vec<u8>> {
    return rasterize_svg_pixmap(svg)?
        .encode_png()
        .map_err(|error| return render_error(format!("encode semantic PNG: {error}")));
}

fn rasterize_svg_pixmap(svg: &[u8]) -> Result<Pixmap> {
    let options = Options {
        fontdb: Arc::clone(semantic_font_database()),
        ..Options::default()
    };
    let tree = Tree::from_data(svg, &options)
        .map_err(|error| return render_error(format!("parse deterministic SVG: {error}")))?;
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height())
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;
    resvg::render(&tree, Transform::default(), &mut pixmap.as_mut());
    return Ok(pixmap);
}

pub(super) fn validate_geometry(cols: u16, rows: u16, label: &str) -> Result<()> {
    if supported_geometry_cell_count(cols, rows).is_none() {
        return Err(render_error(format!(
            "{label} geometry {cols}x{rows} exceeds terminal rendering bounds"
        )));
    }
    return Ok(());
}

fn validate_canvas_geometry(cols: u16, rows: u16) -> Result<()> {
    if cols == 0 || rows == 0 || cols > MAX_TERMINAL_COLUMNS || rows > MAX_TERMINAL_ROWS {
        return Err(render_error(format!(
            "semantic video canvas geometry {cols}x{rows} exceeds terminal rendering bounds"
        )));
    }
    return Ok(());
}
