//! Deterministic semantic-state SVG rendering.

use std::{
    fmt::Write as _,
    mem,
    sync::{Arc, OnceLock},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{Options, Tree, fontdb::Database},
};

use crate::{
    Error, Result,
    diff::SemanticDiff,
    ghostty_vt::{CellStyle, ColorSpec, CursorStyle, Rgb, SemanticState},
};

/// Stable semantic SVG renderer identity.
pub const SEMANTIC_SVG_RENDERER: &str = "termtrace-semantic-svg-v1";
/// Stable semantic PNG rasterizer identity.
pub const SEMANTIC_PNG_RENDERER: &str = "termtrace-semantic-png-resvg-v1";
/// Bundled deterministic font family.
pub const SEMANTIC_FONT_FAMILY: &str = "Termtrace Fira Code";
const SEMANTIC_RASTER_FONT_FAMILY: &str = "Fira Code";
/// Repository-relative source of the bundled font.
pub const SEMANTIC_FONT_SOURCE: &str = "vendor/ghostty/pkg/freetype/res/FiraCode-Regular.ttf";
/// Exact bundled font bytes used by SVG and PNG output.
pub const SEMANTIC_FONT_BYTES: &[u8] =
    include_bytes!("../vendor/ghostty/pkg/freetype/res/FiraCode-Regular.ttf");
/// Deterministic semantic cell width in pixels.
pub const SEMANTIC_CELL_WIDTH: u16 = 10;
/// Deterministic semantic cell height in pixels.
pub const SEMANTIC_CELL_HEIGHT: u16 = 20;

/// Render a complete semantic state as a deterministic SVG scene.
#[must_use]
pub fn render_svg(state: &SemanticState) -> Vec<u8> {
    return render_svg_scene(state, state.cols, state.rows, true);
}

fn render_svg_scene(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
    embed_font: bool,
) -> Vec<u8> {
    const CELL_WIDTH: u32 = SEMANTIC_CELL_WIDTH as u32;
    const CELL_HEIGHT: u32 = SEMANTIC_CELL_HEIGHT as u32;
    const BASELINE: u32 = 15;
    let width = u32::from(canvas_cols).saturating_mul(CELL_WIDTH);
    let height = u32::from(canvas_rows).saturating_mul(CELL_HEIGHT);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" data-truth-layer=\"semantic_rendering\" data-renderer=\"{SEMANTIC_SVG_RENDERER}\">\n"
    );
    let font_family = if embed_font {
        SEMANTIC_FONT_FAMILY
    } else {
        SEMANTIC_RASTER_FONT_FAMILY
    };
    if embed_font {
        let font = STANDARD.encode(SEMANTIC_FONT_BYTES);
        let _ = writeln!(
            svg,
            "<style>@font-face{{font-family:'{font_family}';src:url(data:font/ttf;base64,{font}) format('truetype');font-weight:normal;font-style:normal}}text{{font-family:'{font_family}'}}</style>"
        );
    }
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
                "<text x=\"{x}\" y=\"{text_y}\" fill=\"{}\" font-family=\"{font_family}\" font-size=\"16\" font-weight=\"{weight}\" font-style=\"{font_style}\" opacity=\"{opacity}\" text-decoration=\"{decoration}\" xml:space=\"preserve\">{escaped}</text>",
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

/// Rasterize the exact deterministic SVG scene with only the bundled font.
///
/// # Errors
///
/// Returns an error when the SVG cannot be parsed or the PNG cannot be encoded.
pub fn render_png(state: &SemanticState) -> Result<Vec<u8>> {
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
/// Returns an error when the canvas is smaller than the semantic viewport or
/// the SVG scene cannot be rasterized.
pub fn render_video_rgba(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
) -> Result<Vec<u8>> {
    if canvas_cols < state.cols || canvas_rows < state.rows {
        return Err(image_error(format!(
            "semantic video canvas {canvas_cols}x{canvas_rows} is smaller than state {}x{}",
            state.cols, state.rows
        )));
    }
    let pixmap = rasterize_svg_pixmap(&render_svg_scene(state, canvas_cols, canvas_rows, false))?;
    return Ok(pixmap.data().to_vec());
}

/// Compose before/after semantic scenes and outline every changed later cell.
#[must_use]
pub fn render_diff_svg(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
) -> Vec<u8> {
    const CELL_WIDTH: u32 = 10;
    const CELL_HEIGHT: u32 = 20;
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
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" data-truth-layer=\"semantic_rendering\" data-renderer=\"termtrace-semantic-diff-svg-v1\">\n<style>@font-face{{font-family:'{SEMANTIC_FONT_FAMILY}';src:url(data:font/ttf;base64,{font}) format('truetype');font-weight:normal;font-style:normal}}text{{font-family:'{SEMANTIC_FONT_FAMILY}'}}</style>\n<rect width=\"100%\" height=\"100%\" fill=\"#202020\"/>\n<text x=\"4\" y=\"20\" fill=\"white\" font-size=\"14\">before</text>\n<text x=\"{}\" y=\"20\" fill=\"white\" font-size=\"14\">after</text>\n<image x=\"0\" y=\"{HEADER}\" width=\"{before_width}\" href=\"data:image/svg+xml;base64,{before_svg}\"/>\n<image x=\"{before_width}\" y=\"{HEADER}\" width=\"{after_width}\" href=\"data:image/svg+xml;base64,{after_svg}\"/>\n",
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
/// Returns an error when SVG parsing or PNG encoding fails.
pub fn render_diff_png(
    before: &SemanticState,
    after: &SemanticState,
    difference: &SemanticDiff,
) -> Result<Vec<u8>> {
    return rasterize_svg(&render_diff_svg(before, after, difference));
}

fn rasterize_svg(svg: &[u8]) -> Result<Vec<u8>> {
    return rasterize_svg_pixmap(svg)?
        .encode_png()
        .map_err(|error| return image_error(format!("encode semantic PNG: {error}")));
}

fn rasterize_svg_pixmap(svg: &[u8]) -> Result<Pixmap> {
    let options = Options {
        fontdb: Arc::clone(semantic_font_database()),
        ..Options::default()
    };
    let tree = Tree::from_data(svg, &options).map_err(|error| {
        return image_error(format!("parse deterministic SVG: {error}"));
    })?;
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height())
        .ok_or_else(|| return image_error("allocate semantic PNG surface"))?;
    resvg::render(&tree, Transform::default(), &mut pixmap.as_mut());
    return Ok(pixmap);
}

fn semantic_font_database() -> &'static Arc<Database> {
    static DATABASE: OnceLock<Arc<Database>> = OnceLock::new();
    return DATABASE.get_or_init(|| {
        let mut database = Database::new();
        database.load_font_data(SEMANTIC_FONT_BYTES.to_vec());
        return Arc::new(database);
    });
}

/// BLAKE3 identity of the exact bundled semantic font.
#[must_use]
pub fn semantic_font_hash() -> String {
    return blake3::hash(SEMANTIC_FONT_BYTES).to_hex().to_string();
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

fn image_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
