//! Deterministic semantic before/after diff rendering.

use std::fmt::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use termtrace_model::terminal::diff::SemanticDiff;
use termtrace_model::terminal::state::SemanticState;

use crate::error::Result;
use crate::font::{SEMANTIC_FONT_BYTES, SEMANTIC_FONT_FAMILY};
use crate::raster::{rasterize_svg, validate_geometry};
use crate::semantic_svg::{SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, render_svg};

/// Stable semantic diff SVG renderer identity.
pub const SEMANTIC_DIFF_SVG_RENDERER: &str = "termtrace-semantic-diff-svg-v2";

/// Compose before/after semantic scenes and outline every changed later cell.
#[must_use]
pub fn render_diff_svg(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
) -> Vec<u8> {
    const CELL_WIDTH: u32 = SEMANTIC_CELL_WIDTH as u32;
    const CELL_HEIGHT: u32 = SEMANTIC_CELL_HEIGHT as u32;
    const HEADER: u32 = 30;
    let before_width = u32::from(before.cols).saturating_mul(CELL_WIDTH);
    let after_width = u32::from(after.cols).saturating_mul(CELL_WIDTH);
    let height = u32::from(before.rows.max(after.rows))
        .saturating_mul(CELL_HEIGHT)
        .saturating_add(HEADER);
    let width = before_width.saturating_add(after_width);
    let before_svg = STANDARD.encode(render_svg(before));
    let after_svg = STANDARD.encode(render_svg(after));
    let font = STANDARD.encode(SEMANTIC_FONT_BYTES);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" data-truth-layer=\"semantic_rendering\" data-renderer=\"{SEMANTIC_DIFF_SVG_RENDERER}\">\n<style>@font-face{{font-family:'{SEMANTIC_FONT_FAMILY}';src:url(data:font/ttf;base64,{font}) format('truetype');font-weight:normal;font-style:normal}}text{{font-family:'{SEMANTIC_FONT_FAMILY}'}}</style>\n<rect width=\"100%\" height=\"100%\" fill=\"#202020\"/>\n<text x=\"4\" y=\"20\" fill=\"white\" font-size=\"14\">before</text>\n<text x=\"{}\" y=\"20\" fill=\"white\" font-size=\"14\">after</text>\n<image x=\"0\" y=\"{HEADER}\" width=\"{before_width}\" href=\"data:image/svg+xml;base64,{before_svg}\"/>\n<image x=\"{before_width}\" y=\"{HEADER}\" width=\"{after_width}\" href=\"data:image/svg+xml;base64,{after_svg}\"/>\n",
        before_width.saturating_add(4),
    );
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
    return svg.into_bytes();
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
    return rasterize_svg(&render_diff_svg(before, after, difference));
}
