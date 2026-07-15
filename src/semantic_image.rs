//! Deterministic semantic-state SVG rendering.

use std::{fmt::Write as _, mem};

use crate::ghostty_vt::{CellStyle, ColorSpec, CursorStyle, Rgb, SemanticState};

/// Stable semantic SVG renderer identity.
pub const SEMANTIC_SVG_RENDERER: &str = "termtrace-semantic-svg-v1";

/// Render a complete semantic state as a deterministic SVG scene.
#[must_use]
pub fn render_svg(state: &SemanticState) -> Vec<u8> {
    const CELL_WIDTH: u32 = 10;
    const CELL_HEIGHT: u32 = 20;
    const BASELINE: u32 = 15;
    let width = u32::from(state.cols).saturating_mul(CELL_WIDTH);
    let height = u32::from(state.rows).saturating_mul(CELL_HEIGHT);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" data-truth-layer=\"semantic_rendering\" data-renderer=\"{SEMANTIC_SVG_RENDERER}\">\n"
    );
    let _ = writeln!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>",
        color_hex(state.colors.background)
    );
    for row in &state.grid {
        for cell in &row.cells {
            let mut foreground = resolve_color(
                cell.style.foreground,
                state.colors.foreground,
                &state.colors.palette,
            );
            let mut background = resolve_color(
                cell.style.background,
                state.colors.background,
                &state.colors.palette,
            );
            if cell.style.inverse {
                mem::swap(&mut foreground, &mut background);
            }
            let x = u32::from(cell.column).saturating_mul(CELL_WIDTH);
            let y = u32::from(row.row).saturating_mul(CELL_HEIGHT);
            if background != state.colors.background || cell.style.inverse {
                let _ = writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"{}\"/>",
                    color_hex(background)
                );
            }
            if cell.grapheme.is_empty() || cell.style.invisible {
                continue;
            }
            let weight = if cell.style.bold { "bold" } else { "normal" };
            let font_style = if cell.style.italic {
                "italic"
            } else {
                "normal"
            };
            let opacity = if cell.style.faint { "0.6" } else { "1" };
            let decoration = text_decoration(&cell.style);
            let escaped = escape_xml(&cell.grapheme);
            let text_y = y.saturating_add(BASELINE);
            let _ = writeln!(
                svg,
                "<text x=\"{x}\" y=\"{text_y}\" fill=\"{}\" font-family=\"monospace\" font-size=\"16\" font-weight=\"{weight}\" font-style=\"{font_style}\" opacity=\"{opacity}\" text-decoration=\"{decoration}\" xml:space=\"preserve\">{escaped}</text>",
                color_hex(foreground)
            );
        }
    }
    if state.cursor.visible
        && let (Some(column), Some(row)) = (state.cursor.column, state.cursor.row)
    {
        let x = u32::from(column).saturating_mul(CELL_WIDTH);
        let y = u32::from(row).saturating_mul(CELL_HEIGHT);
        let cursor = state.colors.cursor.unwrap_or(state.colors.foreground);
        match state.cursor.style {
            CursorStyle::Bar => {
                let _ = writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"2\" height=\"{CELL_HEIGHT}\" fill=\"{}\"/>",
                    color_hex(cursor)
                );
            }
            CursorStyle::Underline => {
                let cursor_y = y.saturating_add(CELL_HEIGHT.saturating_sub(2));
                let _ = writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{cursor_y}\" width=\"{CELL_WIDTH}\" height=\"2\" fill=\"{}\"/>",
                    color_hex(cursor)
                );
            }
            CursorStyle::Block => {
                let _ = writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"{}\" opacity=\"0.55\"/>",
                    color_hex(cursor)
                );
            }
            CursorStyle::HollowBlock => {
                let _ = writeln!(
                    svg,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"none\" stroke=\"{}\"/>",
                    color_hex(cursor)
                );
            }
        }
    }
    svg.push_str("</svg>\n");
    return svg.into_bytes();
}

fn resolve_color(spec: ColorSpec, default: Rgb, palette: &[Rgb]) -> Rgb {
    return match spec {
        ColorSpec::None => default,
        ColorSpec::Rgb(color) => color,
        ColorSpec::Palette(index) => palette.get(usize::from(index)).copied().unwrap_or(default),
    };
}

fn color_hex(color: Rgb) -> String {
    return format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
}

fn text_decoration(style: &CellStyle) -> &'static str {
    return match (style.underline != 0, style.strikethrough, style.overline) {
        (false, false, false) => "none",
        (true, false, false) => "underline",
        (false, true, false) => "line-through",
        (false, false, true) => "overline",
        (true, true, false) => "underline line-through",
        (true, false, true) => "underline overline",
        (false, true, true) => "line-through overline",
        (true, true, true) => "underline line-through overline",
    };
}

fn escape_xml(value: &str) -> String {
    return value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
}
