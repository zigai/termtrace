//! Deterministic semantic-state SVG scene construction.

use std::fmt::{self, Display, Formatter, Write};
use std::mem;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use termtrace_model::artifact::save::SemanticFontRole;
use termtrace_model::terminal::state::{
    CellState, CellStyle, ColorSpec, CursorStyle, Rgb, SemanticState, WideKind,
};

use crate::error::Result;
use crate::font::{
    SEMANTIC_RASTER_SYMBOLS_FONT_FAMILY, SEMANTIC_RASTER_TEXT_FONT_FAMILY,
    SEMANTIC_SYMBOLS_FONT_FAMILY, SEMANTIC_TEXT_FONT_FAMILY, SemanticFontSet, semantic_font_set,
};

/// Stable semantic SVG renderer identity.
pub const SEMANTIC_SVG_RENDERER: &str = "termtrace-semantic-svg-v4";
/// Deterministic semantic cell width in pixels.
pub const SEMANTIC_CELL_WIDTH: u16 = 12;
/// Deterministic semantic cell height in pixels.
pub const SEMANTIC_CELL_HEIGHT: u16 = 24;
/// Deterministic semantic text font size in pixels.
pub const SEMANTIC_FONT_SIZE: u16 = 20;
/// Deterministic semantic text baseline in pixels.
pub const SEMANTIC_TEXT_BASELINE: u16 = 20;
/// Deterministic semantic symbols fallback font size in pixels.
pub const SEMANTIC_SYMBOLS_FONT_SIZE: u16 = 12;
/// Deterministic semantic symbols fallback baseline in pixels.
pub const SEMANTIC_SYMBOLS_BASELINE: u16 = 16;

/// Render a complete semantic state as a deterministic SVG scene.
///
/// # Errors
///
/// Returns an error when the bundled semantic font set cannot be initialized.
pub fn render_svg(state: &SemanticState) -> Result<Vec<u8>> {
    return render_svg_scene(state, state.cols, state.rows, true);
}

pub(super) fn render_svg_scene(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
    embed_font: bool,
) -> Result<Vec<u8>> {
    const CELL_WIDTH: u32 = SEMANTIC_CELL_WIDTH as u32;
    const CELL_HEIGHT: u32 = SEMANTIC_CELL_HEIGHT as u32;
    let width = u32::from(canvas_cols).saturating_mul(CELL_WIDTH);
    let height = u32::from(canvas_rows).saturating_mul(CELL_HEIGHT);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" role=\"img\" data-truth-layer=\"semantic_rendering\" data-renderer=\"{SEMANTIC_SVG_RENDERER}\">\n"
    );
    let font_set = semantic_font_set()?;
    if embed_font {
        write_font_faces(&mut svg, font_set);
    }
    write_scene_content(&mut svg, state, font_set, embed_font);
    svg.push_str("</svg>\n");
    return Ok(svg.into_bytes());
}

pub(super) fn write_scene_content(
    svg: &mut String,
    state: &SemanticState,
    font_set: &SemanticFontSet,
    embed_font: bool,
) {
    let _ = writeln!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>",
        HexColor(state.colors.background)
    );
    for row in &state.grid {
        for cell in &row.cells {
            let Some(resolved) = resolve_cell(state, font_set, row.row, cell, embed_font) else {
                continue;
            };
            write_cell(svg, state.colors.background, &resolved);
        }
    }
    write_cursor(svg, state, font_set, embed_font);
}

struct ResolvedCell<'cell> {
    cell: &'cell CellState,
    x: u32,
    y: u32,
    span: u32,
    foreground: Rgb,
    background: Rgb,
    underline_color: Rgb,
    font_family: &'static str,
    font_size: u16,
    baseline: u16,
    weight: &'static str,
    font_style: &'static str,
    opacity: &'static str,
    shape_rendering: &'static str,
}

fn resolve_cell<'cell>(
    state: &SemanticState,
    font_set: &SemanticFontSet,
    row: u16,
    cell: &'cell CellState,
    embed_font: bool,
) -> Option<ResolvedCell<'cell>> {
    let span = match cell.wide_kind {
        WideKind::Narrow => u32::from(SEMANTIC_CELL_WIDTH),
        WideKind::Wide => u32::from(SEMANTIC_CELL_WIDTH).saturating_mul(2),
        WideKind::SpacerTail | WideKind::SpacerHead => return None,
    };
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
    let underline_color = resolve_color(
        cell.style.underline_color,
        foreground,
        &state.colors.palette,
    );
    let role = select_font_role(font_set, &cell.style, &cell.grapheme);
    let (font_size, baseline) = font_geometry(role);
    let (font_family, weight, font_style) = font_attributes(role, embed_font);
    return Some(ResolvedCell {
        cell,
        x: u32::from(cell.column).saturating_mul(u32::from(SEMANTIC_CELL_WIDTH)),
        y: u32::from(row).saturating_mul(u32::from(SEMANTIC_CELL_HEIGHT)),
        span,
        foreground,
        background,
        font_family,
        font_size,
        baseline,
        weight,
        font_style,
        opacity: if cell.style.faint { "0.6" } else { "1" },
        underline_color,
        shape_rendering: if is_terminal_graphic(&cell.grapheme) {
            " shape-rendering=\"crispEdges\""
        } else {
            ""
        },
    });
}

fn write_cell(svg: &mut String, canvas_background: Rgb, resolved: &ResolvedCell<'_>) {
    if resolved.background != canvas_background || resolved.cell.style.inverse {
        let _ = writeln!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            resolved.x,
            resolved.y,
            resolved.span,
            SEMANTIC_CELL_HEIGHT,
            HexColor(resolved.background)
        );
    }
    if resolved.cell.style.invisible {
        return;
    }
    if !resolved.cell.grapheme.is_empty() {
        write_glyph(svg, resolved, resolved.foreground);
    }
    write_decorations(svg, resolved);
}

fn write_glyph(svg: &mut String, resolved: &ResolvedCell<'_>, color: Rgb) {
    let escaped = EscapedXml(&resolved.cell.grapheme);
    let text_y = resolved.y.saturating_add(u32::from(resolved.baseline));
    let _ = writeln!(
        svg,
        "<text x=\"{}\" y=\"{text_y}\" fill=\"{}\" font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\" font-style=\"{}\" opacity=\"{}\"{} xml:space=\"preserve\">{escaped}</text>",
        resolved.x,
        HexColor(color),
        resolved.font_family,
        resolved.font_size,
        resolved.weight,
        resolved.font_style,
        resolved.opacity,
        resolved.shape_rendering
    );
}

fn write_decorations(svg: &mut String, resolved: &ResolvedCell<'_>) {
    match resolved.cell.style.underline {
        1 => write_decoration_line(svg, resolved, 23, resolved.underline_color, ""),
        2 => {
            write_decoration_line(svg, resolved, 21, resolved.underline_color, "");
            write_decoration_line(svg, resolved, 23, resolved.underline_color, "");
        }
        3 => write_curly_underline(svg, resolved),
        4 => write_decoration_line(
            svg,
            resolved,
            23,
            resolved.underline_color,
            " stroke-dasharray=\"1 2\" stroke-linecap=\"round\"",
        ),
        5 => write_decoration_line(
            svg,
            resolved,
            23,
            resolved.underline_color,
            " stroke-dasharray=\"4 2\"",
        ),
        _ => {}
    }
    if resolved.cell.style.strikethrough {
        write_decoration_line(svg, resolved, 14, resolved.foreground, "");
    }
    if resolved.cell.style.overline {
        write_decoration_line(svg, resolved, 1, resolved.foreground, "");
    }
}

fn write_decoration_line(
    svg: &mut String,
    resolved: &ResolvedCell<'_>,
    y_offset: u32,
    color: Rgb,
    attributes: &str,
) {
    let y = resolved.y.saturating_add(y_offset);
    let x_end = resolved.x.saturating_add(resolved.span);
    let _ = writeln!(
        svg,
        "<line x1=\"{}\" y1=\"{y}\" x2=\"{x_end}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"{attributes}/>",
        resolved.x,
        HexColor(color),
        resolved.opacity
    );
}

fn write_curly_underline(svg: &mut String, resolved: &ResolvedCell<'_>) {
    let y = resolved.y.saturating_add(22);
    let upper_y = y.saturating_sub(1);
    let lower_y = y.saturating_add(1);
    let mut path = format!("M {} {y}", resolved.x);
    for start in (resolved.x..resolved.x.saturating_add(resolved.span)).step_by(3) {
        let _ = write!(
            path,
            " Q {start}.75 {upper_y} {}.5 {y} Q {}.25 {lower_y} {} {y}",
            start.saturating_add(1),
            start.saturating_add(2),
            start.saturating_add(3)
        );
    }
    let _ = writeln!(
        svg,
        "<path d=\"{path}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
        HexColor(resolved.underline_color),
        resolved.opacity
    );
}

fn write_cursor(
    svg: &mut String,
    state: &SemanticState,
    font_set: &SemanticFontSet,
    embed_font: bool,
) {
    if !state.cursor.visible || !state.cursor.in_viewport {
        return;
    }
    let (Some(column), Some(row)) = (state.cursor.column, state.cursor.row) else {
        return;
    };
    let is_wide_tail = state.cursor.wide_tail == Some(true);
    let column = if is_wide_tail {
        let Some(lead_column) = column.checked_sub(1) else {
            return;
        };
        lead_column
    } else {
        column
    };
    let Some(row_state) = state
        .grid
        .iter()
        .find(|candidate| return candidate.row == row)
    else {
        return;
    };
    let Some(cell) = row_state
        .cells
        .iter()
        .find(|candidate| return candidate.column == column)
    else {
        return;
    };
    let Some(resolved) = resolve_cell(state, font_set, row, cell, embed_font) else {
        return;
    };
    let cursor_color = state.colors.cursor.unwrap_or(state.colors.foreground);
    match state.cursor.style {
        CursorStyle::Bar => {
            let _ = writeln!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"2\" height=\"{}\" fill=\"{}\"/>",
                resolved.x,
                resolved.y,
                SEMANTIC_CELL_HEIGHT,
                HexColor(cursor_color)
            );
        }
        CursorStyle::Underline => {
            let cursor_y = resolved
                .y
                .saturating_add(u32::from(SEMANTIC_CELL_HEIGHT.saturating_sub(2)));
            let _ = writeln!(
                svg,
                "<rect x=\"{}\" y=\"{cursor_y}\" width=\"{}\" height=\"2\" fill=\"{}\"/>",
                resolved.x,
                SEMANTIC_CELL_WIDTH,
                HexColor(cursor_color)
            );
        }
        CursorStyle::Block => {
            let span = if is_wide_tail || cell.wide_kind == WideKind::Wide {
                u32::from(SEMANTIC_CELL_WIDTH).saturating_mul(2)
            } else {
                u32::from(SEMANTIC_CELL_WIDTH)
            };
            let _ = writeln!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"{span}\" height=\"{}\" fill=\"{}\"/>",
                resolved.x,
                resolved.y,
                SEMANTIC_CELL_HEIGHT,
                HexColor(cursor_color)
            );
            if !cell.grapheme.is_empty() && !cell.style.invisible {
                write_glyph(svg, &resolved, resolved.background);
            }
        }
        CursorStyle::HollowBlock => {
            let x = f64::from(resolved.x) + 0.5;
            let y = f64::from(resolved.y) + 0.5;
            let width = SEMANTIC_CELL_WIDTH.saturating_sub(1);
            let height = SEMANTIC_CELL_HEIGHT.saturating_sub(1);
            let _ = writeln!(
                svg,
                "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\"/>",
                HexColor(cursor_color)
            );
        }
    }
}

fn is_terminal_graphic(grapheme: &str) -> bool {
    let mut characters = grapheme.chars();
    let Some(character) = characters.next() else {
        return false;
    };
    if characters.next().is_some() {
        return false;
    }
    return ('\u{2500}'..='\u{257f}').contains(&character)
        || ('\u{2580}'..='\u{259f}').contains(&character);
}

pub(super) fn write_font_faces(svg: &mut String, font_set: &SemanticFontSet) {
    svg.push_str("<style>");
    for resource in font_set.resources() {
        let role = resource.role;
        let (family, weight, style) = font_attributes(role, true);
        let _ = write!(
            svg,
            "@font-face{{font-family:'{family}';src:url(data:font/ttf;base64,"
        );
        STANDARD.encode_string(font_set.face_bytes(role), svg);
        let _ = write!(
            svg,
            ") format('truetype');font-weight:{weight};font-style:{style}}}"
        );
    }
    svg.push_str("</style>\n");
}

pub(super) fn select_font_role(
    font_set: &SemanticFontSet,
    style: &CellStyle,
    grapheme: &str,
) -> SemanticFontRole {
    let primary = match (style.bold, style.italic) {
        (false, false) => SemanticFontRole::Regular,
        (true, false) => SemanticFontRole::Bold,
        (false, true) => SemanticFontRole::Italic,
        (true, true) => SemanticFontRole::BoldItalic,
    };
    if font_set.supports_grapheme(primary, grapheme) {
        return primary;
    }
    if font_set.supports_grapheme(SemanticFontRole::SymbolsFallback, grapheme) {
        return SemanticFontRole::SymbolsFallback;
    }
    return primary;
}

fn font_geometry(role: SemanticFontRole) -> (u16, u16) {
    if role == SemanticFontRole::SymbolsFallback {
        return (SEMANTIC_SYMBOLS_FONT_SIZE, SEMANTIC_SYMBOLS_BASELINE);
    }
    return (SEMANTIC_FONT_SIZE, SEMANTIC_TEXT_BASELINE);
}

fn font_attributes(
    role: SemanticFontRole,
    embedded: bool,
) -> (&'static str, &'static str, &'static str) {
    match (role, embedded) {
        (SemanticFontRole::Regular, true) => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "normal", "normal");
        }
        (SemanticFontRole::Bold, true) => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "bold", "normal");
        }
        (SemanticFontRole::Italic, true) => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "normal", "italic");
        }
        (SemanticFontRole::BoldItalic, true) => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "bold", "italic");
        }
        (SemanticFontRole::SymbolsFallback, true) => {
            return (SEMANTIC_SYMBOLS_FONT_FAMILY, "normal", "normal");
        }
        (SemanticFontRole::Regular, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_FAMILY, "normal", "normal");
        }
        (SemanticFontRole::Bold, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_FAMILY, "bold", "normal");
        }
        (SemanticFontRole::Italic, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_FAMILY, "normal", "italic");
        }
        (SemanticFontRole::BoldItalic, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_FAMILY, "bold", "italic");
        }
        (SemanticFontRole::SymbolsFallback, false) => {
            return (SEMANTIC_RASTER_SYMBOLS_FONT_FAMILY, "normal", "normal");
        }
    }
}

fn resolve_color(spec: ColorSpec, default: Rgb, palette: &[Rgb]) -> Rgb {
    match spec {
        ColorSpec::None => return default,
        ColorSpec::Rgb(color) => return color,
        ColorSpec::Palette(index) => {
            return palette.get(usize::from(index)).copied().unwrap_or(default);
        }
    }
}

#[derive(Clone, Copy)]
struct HexColor(Rgb);

impl Display for HexColor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return write!(f, "#{:02x}{:02x}{:02x}", self.0.r, self.0.g, self.0.b);
    }
}

struct EscapedXml<'text>(&'text str);

impl Display for EscapedXml<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '&' => f.write_str("&amp;")?,
                '<' => f.write_str("&lt;")?,
                '>' => f.write_str("&gt;")?,
                '"' => f.write_str("&quot;")?,
                '\'' => f.write_str("&apos;")?,
                _ => f.write_char(ch)?,
            }
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::render_png;
    use termtrace_terminal::GhosttyVt;

    #[test]
    fn style_and_coverage_select_exact_bundled_faces() {
        let font_set = semantic_font_set().expect("initialize bundled semantic font set");
        let mut terminal = GhosttyVt::new(1, 1, 0).expect("create terminal");
        terminal.write(b"A");
        let state = terminal.snapshot().expect("snapshot terminal");
        let [row] = state.grid.as_slice() else {
            panic!("one-row terminal must produce one row");
        };
        let [cell] = row.cells.as_slice() else {
            panic!("one-column terminal must produce one cell");
        };
        let regular = cell.style.clone();
        let mut bold = regular.clone();
        bold.bold = true;
        let mut italic = regular.clone();
        italic.italic = true;
        let mut bold_italic = regular.clone();
        bold_italic.bold = true;
        bold_italic.italic = true;

        assert_eq!(
            select_font_role(font_set, &regular, "A"),
            SemanticFontRole::Regular
        );
        assert_eq!(
            select_font_role(font_set, &bold, "A"),
            SemanticFontRole::Bold
        );
        assert_eq!(
            select_font_role(font_set, &italic, "A"),
            SemanticFontRole::Italic
        );
        assert_eq!(
            select_font_role(font_set, &bold_italic, "A"),
            SemanticFontRole::BoldItalic
        );
        for icon in ["\u{f322}", "\u{f0c3}"] {
            assert_eq!(
                select_font_role(font_set, &bold_italic, icon),
                SemanticFontRole::SymbolsFallback
            );
        }
        assert_eq!(
            select_font_role(font_set, &regular, "\u{30000}"),
            SemanticFontRole::Regular
        );
    }

    #[test]
    fn cached_bundle_renders_byte_identical_svg_and_png() {
        let mut terminal = GhosttyVt::new(1, 1, 0).expect("create terminal");
        terminal.write(b"A");
        let mut state = terminal.snapshot().expect("snapshot terminal");
        let [row] = state.grid.as_mut_slice() else {
            panic!("one-row terminal must produce one row");
        };
        let [cell] = row.cells.as_mut_slice() else {
            panic!("one-column terminal must produce one cell");
        };
        cell.grapheme = "\u{30000}".to_owned();

        let first_svg = render_svg(&state).expect("render first SVG");
        let second_svg = render_svg(&state).expect("render second SVG");
        assert_eq!(first_svg, second_svg);
        let svg = String::from_utf8(first_svg).expect("semantic SVG is UTF-8");

        assert_eq!(svg.matches("@font-face").count(), 5);

        let first_png = render_png(&state).expect("render first PNG");
        let second_png = render_png(&state).expect("render second PNG");
        assert_eq!(first_png, second_png);
    }
    #[test]
    fn semantic_layout_constants_match_bundled_font_metrics() {
        let font_set = semantic_font_set().expect("load font set");
        let regular =
            ttf_parser::Face::parse(font_set.regular_face_bytes(), 0).expect("parse regular font");
        let advance = regular
            .glyph_index('A')
            .and_then(|g| return regular.glyph_hor_advance(g))
            .expect("glyph advance");
        let scaled_advance = (f32::from(advance) * (SEMANTIC_FONT_SIZE as f32)
            / f32::from(regular.units_per_em()))
        .round() as u16;
        assert_eq!(scaled_advance, SEMANTIC_CELL_WIDTH);

        let symbols =
            ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::SymbolsFallback), 0)
                .expect("parse symbols font");
        let symbols_height = (f32::from(symbols.ascender() - symbols.descender())
            * (SEMANTIC_SYMBOLS_FONT_SIZE as f32)
            / f32::from(symbols.units_per_em()))
        .round() as u16;
        assert!(symbols_height <= SEMANTIC_CELL_HEIGHT);
    }
    #[test]
    fn terminal_graphics_require_one_scalar_in_a_terminal_graphics_range() {
        for grapheme in ["─", "╿", "▀", "▟"] {
            assert!(is_terminal_graphic(grapheme));
        }
        for grapheme in ["A", "──", "▀\u{fe0f}", ""] {
            assert!(!is_terminal_graphic(grapheme));
        }
    }
}
