//! Deterministic semantic before/after diff rendering.

use crate::error::Result;
use crate::font::{semantic_font_set, SEMANTIC_RASTER_TEXT_FONT_FAMILY, SEMANTIC_TEXT_FONT_FAMILY};
use crate::raster::{rasterize_svg, validate_geometry};
use crate::semantic_svg::{
    write_font_faces, write_scene_content, SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH,
};
use std::fmt::Write;
use termtrace_model::terminal::diff::SemanticDiff;
use termtrace_model::terminal::state::SemanticState;

/// Stable semantic diff SVG renderer identity.
pub const SEMANTIC_DIFF_SVG_RENDERER: &str = "termtrace-semantic-diff-svg-v4";

/// Compose before/after semantic scenes and outline every changed later cell.
///
/// # Errors
///
/// Returns an error when the bundled semantic font set cannot be initialized.
pub fn render_diff_svg(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
) -> Result<Vec<u8>> {
    return render_diff_scene(before, after, difference, true);
}

fn render_diff_scene(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
    embed_font: bool,
) -> Result<Vec<u8>> {
    const CELL_WIDTH: u32 = SEMANTIC_CELL_WIDTH as u32;
    const CELL_HEIGHT: u32 = SEMANTIC_CELL_HEIGHT as u32;
    const HEADER: u32 = 30;

    let before_width = u32::from(before.cols).saturating_mul(CELL_WIDTH);
    let after_width = u32::from(after.cols).saturating_mul(CELL_WIDTH);
    let height = u32::from(before.rows.max(after.rows))
        .saturating_mul(CELL_HEIGHT)
        .saturating_add(HEADER);
    let width = before_width.saturating_add(after_width);

    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" data-truth-layer=\"semantic_rendering\" data-renderer=\"{SEMANTIC_DIFF_SVG_RENDERER}\">\n"
    );

    let font_set = semantic_font_set()?;
    let label_font = if embed_font {
        write_font_faces(&mut svg, font_set);
        SEMANTIC_TEXT_FONT_FAMILY
    } else {
        SEMANTIC_RASTER_TEXT_FONT_FAMILY
    };

    let after_label_x = before_width.saturating_add(4);
    let _ = write!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"#202020\"/>\n<text x=\"4\" y=\"20\" fill=\"white\" font-family=\"{label_font}\" font-size=\"14\">before</text>\n<text x=\"{after_label_x}\" y=\"20\" fill=\"white\" font-family=\"{label_font}\" font-size=\"14\">after</text>\n"
    );

    for (state, x, viewport_width) in [
        (before, 0, before_width),
        (after, before_width, after_width),
    ] {
        let viewport_height = u32::from(state.rows).saturating_mul(CELL_HEIGHT);
        let _ = writeln!(
            svg,
            "<svg x=\"{x}\" y=\"{HEADER}\" width=\"{viewport_width}\" height=\"{viewport_height}\" viewBox=\"0 0 {viewport_width} {viewport_height}\" overflow=\"hidden\">"
        );
        write_scene_content(&mut svg, state, font_set, embed_font);
        svg.push_str("</svg>\n");
    }

    for row in &difference.rows {
        for cell in &row.cells {
            let x = before_width.saturating_add(u32::from(cell.column).saturating_mul(CELL_WIDTH));
            let y = HEADER.saturating_add(u32::from(row.row).saturating_mul(CELL_HEIGHT));
            let _ = writeln!(
                svg,
                "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"none\" stroke=\"#ff3b30\" stroke-width=\"2\"/>"
            );
        }
    }

    svg.push_str("</svg>\n");
    return Ok(svg.into_bytes());
}

/// Rasterize the semantic visual-diff composition.
///
/// # Errors
///
/// Returns an error when either semantic geometry is outside the supported
/// bounds, SVG parsing fails, or PNG encoding fails.
pub fn render_diff_png(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
) -> Result<Vec<u8>> {
    validate_geometry(before.cols, before.rows, "before semantic state")?;
    validate_geometry(after.cols, after.rows, "after semantic state")?;
    return rasterize_svg(&render_diff_scene(before, after, difference, false)?);
}
