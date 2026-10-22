//! Deterministic semantic-state SVG scene construction.

use crate::error::Result;
use crate::font::{
    semantic_font_set, SemanticFontSet, SEMANTIC_RASTER_SYMBOLS_FONT_STACK,
    SEMANTIC_RASTER_TEXT_FONT_STACK, SEMANTIC_SYMBOLS_FONT_STACK, SEMANTIC_TEXT_FONT_STACK,
};
use std::fmt::{self, Display, Formatter, Write};
use std::mem;
use termtrace_model::artifact::save::SemanticFontRole;
use termtrace_model::terminal::state::{
    CellState, CellStyle, ColorSpec, CursorStyle, Rgb, RowState, SemanticState, WideKind,
};

/// Stable semantic SVG renderer identity.
pub const SEMANTIC_SVG_RENDERER: &str = "termtrace-semantic-svg-v5";
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

    write_scene_content(&mut svg, state, font_set, embed_font)?;
    svg.push_str("</svg>\n");

    return Ok(svg.into_bytes());
}

pub(super) fn write_scene_content(
    svg: &mut String,
    state: &SemanticState,
    font_set: &SemanticFontSet,
    embed_font: bool,
) -> Result<()> {
    writeln!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>",
        HexColor(state.colors.background)
    )?;

    for row in &state.grid {
        write_row_content(svg, state, font_set, row, embed_font)?;
    }

    write_cursor(svg, state, font_set, embed_font)?;

    return Ok(());
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

    let mut foreground = cell.resolved_foreground.unwrap_or_else(|| {
        return resolve_color(
            cell.style.foreground,
            state.colors.foreground,
            &state.colors.palette,
        );
    });

    let mut background = cell.resolved_background.unwrap_or_else(|| {
        return resolve_color(
            cell.style.background,
            state.colors.background,
            &state.colors.palette,
        );
    });

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

struct BgRun {
    x: u32,
    y: u32,
    width: u32,
    color: Rgb,
}

struct TextRun {
    x: u32,
    y: u32,
    span: u32,
    is_narrow: bool,
    foreground: Rgb,
    font_family: &'static str,
    font_size: u16,
    baseline: u16,
    weight: &'static str,
    font_style: &'static str,
    opacity: &'static str,
    shape_rendering: &'static str,
    cells: Vec<(u32, String)>,
}

struct UnderlineRun {
    x: u32,
    y: u32,
    span: u32,
    underline: i32,
    color: Rgb,
    opacity: &'static str,
}

struct SimpleLineRun {
    x: u32,
    y: u32,
    span: u32,
    y_offset: u32,
    color: Rgb,
    opacity: &'static str,
}

fn write_row_content(
    svg: &mut String,
    state: &SemanticState,
    font_set: &SemanticFontSet,
    row: &RowState,
    embed_font: bool,
) -> Result<()> {
    let mut resolved_cells = Vec::with_capacity(row.cells.len());
    for cell in row.cells.iter() {
        if let Some(resolved) = resolve_cell(state, font_set, row.row, cell, embed_font) {
            resolved_cells.push(resolved);
        }
    }

    if resolved_cells.is_empty() {
        return Ok(());
    }

    // 1. Coalesced Background Rectangles
    let mut current_bg: Option<BgRun> = None;
    for resolved in &resolved_cells {
        let need_bg = resolved.background != state.colors.background || resolved.cell.style.inverse;
        if need_bg {
            if let Some(run) = current_bg.as_mut()
                && run.color == resolved.background
                && run.x.saturating_add(run.width) == resolved.x
            {
                run.width = run.width.saturating_add(resolved.span);
                continue;
            }

            if let Some(run) = current_bg.take() {
                emit_bg_rect(svg, &run)?;
            }

            current_bg = Some(BgRun {
                x: resolved.x,
                y: resolved.y,
                width: resolved.span,
                color: resolved.background,
            });
        } else if let Some(run) = current_bg.take() {
            emit_bg_rect(svg, &run)?;
        }
    }

    if let Some(run) = current_bg {
        emit_bg_rect(svg, &run)?;
    }

    // 2. Coalesced Text Glyphs
    let mut current_text: Option<TextRun> = None;
    for resolved in &resolved_cells {
        let has_text = !resolved.cell.style.invisible && !resolved.cell.grapheme.is_empty();
        if has_text {
            let is_narrow = resolved.cell.wide_kind == WideKind::Narrow;
            let can_merge = current_text.as_ref().is_some_and(|run| {
                return is_narrow
                    && run.is_narrow
                    && run.x.saturating_add(run.span) == resolved.x
                    && run.foreground == resolved.foreground
                    && run.baseline == resolved.baseline
                    && run.font_size == resolved.font_size
                    && run.font_family == resolved.font_family
                    && run.weight == resolved.weight
                    && run.font_style == resolved.font_style
                    && run.opacity == resolved.opacity
                    && run.shape_rendering == resolved.shape_rendering;
            });

            if can_merge && let Some(run) = current_text.as_mut() {
                run.cells
                    .push((resolved.x, resolved.cell.grapheme.to_string()));

                run.span = run.span.saturating_add(resolved.span);
                continue;
            }

            if let Some(run) = current_text.take() {
                emit_text_run(svg, &run)?;
            }

            current_text = Some(TextRun {
                x: resolved.x,
                y: resolved.y,
                span: resolved.span,
                is_narrow,
                foreground: resolved.foreground,
                font_family: resolved.font_family,
                font_size: resolved.font_size,
                baseline: resolved.baseline,
                weight: resolved.weight,
                font_style: resolved.font_style,
                opacity: resolved.opacity,
                shape_rendering: resolved.shape_rendering,
                cells: vec![(resolved.x, resolved.cell.grapheme.to_string())],
            });
        } else if let Some(run) = current_text.take() {
            emit_text_run(svg, &run)?;
        }
    }

    if let Some(run) = current_text {
        emit_text_run(svg, &run)?;
    }

    // 3. Coalesced Decorations
    write_row_decorations(svg, &resolved_cells)?;

    return Ok(());
}

fn write_row_decorations(svg: &mut String, resolved_cells: &[ResolvedCell<'_>]) -> Result<()> {
    // Underlines
    let mut current_underline: Option<UnderlineRun> = None;
    for resolved in resolved_cells {
        let underline = resolved.cell.style.underline;
        if underline != 0 {
            if let Some(run) = current_underline.as_mut()
                && run.underline == underline
                && run.color == resolved.underline_color
                && run.opacity == resolved.opacity
                && run.x.saturating_add(run.span) == resolved.x
            {
                run.span = run.span.saturating_add(resolved.span);
                continue;
            }

            if let Some(run) = current_underline.take() {
                emit_underline_run(svg, &run)?;
            }

            current_underline = Some(UnderlineRun {
                x: resolved.x,
                y: resolved.y,
                span: resolved.span,
                underline,
                color: resolved.underline_color,
                opacity: resolved.opacity,
            });
        } else if let Some(run) = current_underline.take() {
            emit_underline_run(svg, &run)?;
        }
    }

    if let Some(run) = current_underline {
        emit_underline_run(svg, &run)?;
    }

    // Strikethrough
    let mut current_strike: Option<SimpleLineRun> = None;
    for resolved in resolved_cells {
        if resolved.cell.style.strikethrough {
            if let Some(run) = current_strike.as_mut()
                && run.color == resolved.foreground
                && run.opacity == resolved.opacity
                && run.x.saturating_add(run.span) == resolved.x
            {
                run.span = run.span.saturating_add(resolved.span);
                continue;
            }

            if let Some(run) = current_strike.take() {
                emit_simple_line_run(svg, &run)?;
            }

            current_strike = Some(SimpleLineRun {
                x: resolved.x,
                y: resolved.y,
                span: resolved.span,
                y_offset: 14,
                color: resolved.foreground,
                opacity: resolved.opacity,
            });
        } else if let Some(run) = current_strike.take() {
            emit_simple_line_run(svg, &run)?;
        }
    }

    if let Some(run) = current_strike {
        emit_simple_line_run(svg, &run)?;
    }

    // Overline
    let mut current_overline: Option<SimpleLineRun> = None;
    for resolved in resolved_cells {
        if resolved.cell.style.overline {
            if let Some(run) = current_overline.as_mut()
                && run.color == resolved.foreground
                && run.opacity == resolved.opacity
                && run.x.saturating_add(run.span) == resolved.x
            {
                run.span = run.span.saturating_add(resolved.span);
                continue;
            }

            if let Some(run) = current_overline.take() {
                emit_simple_line_run(svg, &run)?;
            }

            current_overline = Some(SimpleLineRun {
                x: resolved.x,
                y: resolved.y,
                span: resolved.span,
                y_offset: 1,
                color: resolved.foreground,
                opacity: resolved.opacity,
            });
        } else if let Some(run) = current_overline.take() {
            emit_simple_line_run(svg, &run)?;
        }
    }

    if let Some(run) = current_overline {
        emit_simple_line_run(svg, &run)?;
    }

    return Ok(());
}

fn emit_bg_rect(svg: &mut String, run: &BgRun) -> Result<()> {
    writeln!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
        run.x,
        run.y,
        run.width,
        SEMANTIC_CELL_HEIGHT,
        HexColor(run.color)
    )?;

    return Ok(());
}

// Each cell gets its own absolute x so renderers shape cells separately and never join
// neighbouring cells into font ligatures such as `=>`.
fn emit_text_run(svg: &mut String, run: &TextRun) -> Result<()> {
    let text_y = run.y.saturating_add(u32::from(run.baseline));
    write!(
        svg,
        "<text y=\"{text_y}\" fill=\"{}\" font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\" font-style=\"{}\" opacity=\"{}\"{} xml:space=\"preserve\">",
        HexColor(run.foreground),
        run.font_family,
        run.font_size,
        run.weight,
        run.font_style,
        run.opacity,
        run.shape_rendering
    )?;

    for (x, grapheme) in &run.cells {
        write!(svg, "<tspan x=\"{x}\">{}</tspan>", EscapedXml(grapheme))?;
    }

    writeln!(svg, "</text>")?;

    return Ok(());
}

fn emit_underline_run(svg: &mut String, run: &UnderlineRun) -> Result<()> {
    let x_end = run.x.saturating_add(run.span);
    match run.underline {
        1 => {
            let y = run.y.saturating_add(23);
            writeln!(
                svg,
                "<line x1=\"{}\" y1=\"{y}\" x2=\"{x_end}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
                run.x,
                HexColor(run.color),
                run.opacity
            )?;
        }
        2 => {
            let y1 = run.y.saturating_add(21);
            let y2 = run.y.saturating_add(23);

            writeln!(
                svg,
                "<line x1=\"{}\" y1=\"{y1}\" x2=\"{x_end}\" y2=\"{y1}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
                run.x,
                HexColor(run.color),
                run.opacity
            )?;
            writeln!(
                svg,
                "<line x1=\"{}\" y1=\"{y2}\" x2=\"{x_end}\" y2=\"{y2}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
                run.x,
                HexColor(run.color),
                run.opacity
            )?;
        }
        3 => {
            let y = run.y.saturating_add(22);
            let upper_y = y.saturating_sub(1);
            let lower_y = y.saturating_add(1);

            let mut path = format!("M {} {y}", run.x);
            for start in (run.x..x_end).step_by(3) {
                write!(
                    path,
                    " Q {start}.75 {upper_y} {}.5 {y} Q {}.25 {lower_y} {} {y}",
                    start.saturating_add(1),
                    start.saturating_add(2),
                    start.saturating_add(3)
                )?;
            }

            writeln!(
                svg,
                "<path d=\"{path}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
                HexColor(run.color),
                run.opacity
            )?;
        }
        4 => {
            let y = run.y.saturating_add(23);
            writeln!(
                svg,
                "<line x1=\"{}\" y1=\"{y}\" x2=\"{x_end}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\" stroke-dasharray=\"1 2\" stroke-linecap=\"round\"/>",
                run.x,
                HexColor(run.color),
                run.opacity
            )?;
        }
        5 => {
            let y = run.y.saturating_add(23);
            writeln!(
                svg,
                "<line x1=\"{}\" y1=\"{y}\" x2=\"{x_end}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\" stroke-dasharray=\"4 2\"/>",
                run.x,
                HexColor(run.color),
                run.opacity
            )?;
        }
        _ => {}
    }

    return Ok(());
}

fn emit_simple_line_run(svg: &mut String, run: &SimpleLineRun) -> Result<()> {
    let y = run.y.saturating_add(run.y_offset);
    let x_end = run.x.saturating_add(run.span);
    writeln!(
        svg,
        "<line x1=\"{}\" y1=\"{y}\" x2=\"{x_end}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\" opacity=\"{}\"/>",
        run.x,
        HexColor(run.color),
        run.opacity
    )?;

    return Ok(());
}

fn write_glyph(svg: &mut String, resolved: &ResolvedCell<'_>, color: Rgb) -> Result<()> {
    let escaped = EscapedXml(&resolved.cell.grapheme);
    let text_y = resolved.y.saturating_add(u32::from(resolved.baseline));
    writeln!(
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
    )?;

    return Ok(());
}

fn write_cursor(
    svg: &mut String,
    state: &SemanticState,
    font_set: &SemanticFontSet,
    embed_font: bool,
) -> Result<()> {
    if !state.cursor.visible || !state.cursor.in_viewport {
        return Ok(());
    }

    let (Some(column), Some(row)) = (state.cursor.column, state.cursor.row) else {
        return Ok(());
    };

    let is_wide_tail = state.cursor.wide_tail == Some(true);
    let column = if is_wide_tail {
        let Some(lead_column) = column.checked_sub(1) else {
            return Ok(());
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
        return Ok(());
    };
    let Some(cell) = row_state
        .cells
        .iter()
        .find(|candidate| return candidate.column == column)
    else {
        return Ok(());
    };
    let Some(resolved) = resolve_cell(state, font_set, row, cell, embed_font) else {
        return Ok(());
    };

    let cursor_color = state.colors.cursor.unwrap_or(state.colors.foreground);

    match state.cursor.style {
        CursorStyle::Bar => {
            writeln!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"2\" height=\"{}\" fill=\"{}\"/>",
                resolved.x,
                resolved.y,
                SEMANTIC_CELL_HEIGHT,
                HexColor(cursor_color)
            )?;
        }
        CursorStyle::Underline => {
            let cursor_y = resolved
                .y
                .saturating_add(u32::from(SEMANTIC_CELL_HEIGHT.saturating_sub(2)));
            writeln!(
                svg,
                "<rect x=\"{}\" y=\"{cursor_y}\" width=\"{}\" height=\"2\" fill=\"{}\"/>",
                resolved.x,
                SEMANTIC_CELL_WIDTH,
                HexColor(cursor_color)
            )?;
        }
        CursorStyle::Block => {
            let span = if is_wide_tail || cell.wide_kind == WideKind::Wide {
                u32::from(SEMANTIC_CELL_WIDTH).saturating_mul(2)
            } else {
                u32::from(SEMANTIC_CELL_WIDTH)
            };
            writeln!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"{span}\" height=\"{}\" fill=\"{}\"/>",
                resolved.x,
                resolved.y,
                SEMANTIC_CELL_HEIGHT,
                HexColor(cursor_color)
            )?;

            if !cell.grapheme.is_empty() && !cell.style.invisible {
                write_glyph(svg, &resolved, resolved.background)?;
            }
        }
        CursorStyle::HollowBlock => {
            let x = f64::from(resolved.x) + 0.5;
            let y = f64::from(resolved.y) + 0.5;
            let width = SEMANTIC_CELL_WIDTH.saturating_sub(1);
            let height = SEMANTIC_CELL_HEIGHT.saturating_sub(1);
            writeln!(
                svg,
                "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\"/>",
                HexColor(cursor_color)
            )?;
        }
    }

    return Ok(());
}

pub(crate) fn is_terminal_graphic(grapheme: &str) -> bool {
    let mut characters = grapheme.chars();
    let Some(character) = characters.next() else {
        return false;
    };

    if characters.next().is_some() {
        return false;
    }
    return ('\u{2500}'..='\u{257f}').contains(&character)
        || ('\u{2580}'..='\u{259f}').contains(&character)
        || ('\u{e0a0}'..='\u{e0a3}').contains(&character)
        || ('\u{e0b0}'..='\u{e0d4}').contains(&character);
}

pub(super) fn write_font_faces(svg: &mut String, font_set: &SemanticFontSet) {
    svg.push_str(font_set.font_faces_css());
}

pub(crate) fn select_font_role(
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

    if primary != SemanticFontRole::Regular
        && font_set.supports_grapheme(SemanticFontRole::Regular, grapheme)
    {
        return SemanticFontRole::Regular;
    }
    return primary;
}

pub(crate) fn font_geometry(role: SemanticFontRole) -> (u16, u16) {
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
            return (SEMANTIC_TEXT_FONT_STACK, "normal", "normal");
        }
        (SemanticFontRole::Bold, true) => {
            return (SEMANTIC_TEXT_FONT_STACK, "bold", "normal");
        }
        (SemanticFontRole::Italic, true) => {
            return (SEMANTIC_TEXT_FONT_STACK, "normal", "italic");
        }
        (SemanticFontRole::BoldItalic, true) => {
            return (SEMANTIC_TEXT_FONT_STACK, "bold", "italic");
        }
        (SemanticFontRole::SymbolsFallback, true) => {
            return (SEMANTIC_SYMBOLS_FONT_STACK, "normal", "normal");
        }
        (SemanticFontRole::Regular, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_STACK, "normal", "normal");
        }
        (SemanticFontRole::Bold, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_STACK, "bold", "normal");
        }
        (SemanticFontRole::Italic, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_STACK, "normal", "italic");
        }
        (SemanticFontRole::BoldItalic, false) => {
            return (SEMANTIC_RASTER_TEXT_FONT_STACK, "bold", "italic");
        }
        (SemanticFontRole::SymbolsFallback, false) => {
            return (SEMANTIC_RASTER_SYMBOLS_FONT_STACK, "normal", "normal");
        }
    }
}

pub(crate) fn resolve_color(spec: ColorSpec, default: Rgb, palette: &[Rgb]) -> Rgb {
    match spec {
        ColorSpec::None => return default,
        ColorSpec::Rgb(color) => return color,
        ColorSpec::Palette(index) => {
            if let Some(color) = palette.get(usize::from(index)).copied() {
                return color;
            }
            return default_ansi256_color(index);
        }
    }
}

/// Standard xterm / ANSI-256 color lookup when the recorded palette does not contain the requested index.
#[must_use]
pub(crate) fn default_ansi256_color(index: u8) -> Rgb {
    match index {
        0 => return Rgb { r: 0, g: 0, b: 0 },
        1 => return Rgb { r: 205, g: 0, b: 0 },
        2 => return Rgb { r: 0, g: 205, b: 0 },
        3 => {
            return Rgb {
                r: 205,
                g: 205,
                b: 0,
            };
        }
        4 => return Rgb { r: 0, g: 0, b: 238 },
        5 => {
            return Rgb {
                r: 205,
                g: 0,
                b: 205,
            };
        }
        6 => {
            return Rgb {
                r: 0,
                g: 205,
                b: 205,
            };
        }
        7 => {
            return Rgb {
                r: 229,
                g: 229,
                b: 229,
            };
        }
        8 => {
            return Rgb {
                r: 127,
                g: 127,
                b: 127,
            };
        }
        9 => return Rgb { r: 255, g: 0, b: 0 },
        10 => return Rgb { r: 0, g: 255, b: 0 },
        11 => {
            return Rgb {
                r: 255,
                g: 255,
                b: 0,
            };
        }
        12 => {
            return Rgb {
                r: 92,
                g: 92,
                b: 255,
            };
        }
        13 => {
            return Rgb {
                r: 255,
                g: 0,
                b: 255,
            };
        }
        14 => {
            return Rgb {
                r: 0,
                g: 255,
                b: 255,
            };
        }
        15 => {
            return Rgb {
                r: 255,
                g: 255,
                b: 255,
            };
        }
        16..=231 => {
            let i = index - 16;
            let r = i / 36;
            let g = (i / 6) % 6;
            let b = i % 6;
            let conv = |v: u8| if v == 0 { return 0 } else { return v * 40 + 55 };

            return Rgb {
                r: conv(r),
                g: conv(g),
                b: conv(b),
            };
        }
        232..=255 => {
            let gray = (index - 232) * 10 + 8;

            return Rgb {
                r: gray,
                g: gray,
                b: gray,
            };
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

fn is_xml_10_char(ch: char) -> bool {
    return matches!(
        ch,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    );
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
                ch if !is_xml_10_char(ch) => f.write_char(' ')?,
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
    use std::sync::Arc;
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
        let [cell] = row.cells.as_ref() else {
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
        let [cell] = Arc::make_mut(&mut row.cells) else {
            panic!("one-column terminal must produce one cell");
        };
        cell.grapheme = "\u{30000}".into();

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
        let scaled_advance = (f32::from(advance) * f32::from(SEMANTIC_FONT_SIZE)
            / f32::from(regular.units_per_em()))
        .round();
        assert!((scaled_advance - f32::from(SEMANTIC_CELL_WIDTH)).abs() < f32::EPSILON);

        let symbols =
            ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::SymbolsFallback), 0)
                .expect("parse symbols font");

        let symbols_height = (f32::from(symbols.ascender() - symbols.descender())
            * f32::from(SEMANTIC_SYMBOLS_FONT_SIZE)
            / f32::from(symbols.units_per_em()))
        .round();
        assert!(symbols_height <= f32::from(SEMANTIC_CELL_HEIGHT));
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

    #[test]
    fn control_characters_sanitize_to_valid_xml() {
        let mut terminal = GhosttyVt::new(5, 1, 0).expect("create terminal");
        terminal.write(b"A\x11\x12\x13B");
        let state = terminal.snapshot().expect("snapshot terminal");
        let svg = render_svg(&state).expect("render SVG with control characters");
        let svg_str = String::from_utf8(svg).expect("valid UTF-8 SVG");
        assert!(!svg_str.contains("\u{11}"));
        assert!(!svg_str.contains("\u{12}"));
        assert!(!svg_str.contains("\u{13}"));

        let _png = render_png(&state).expect("render PNG parses deterministic SVG cleanly");
    }

    #[test]
    fn run_length_coalescing_merges_contiguous_cells_and_backgrounds() {
        let mut terminal = GhosttyVt::new(20, 2, 0).expect("create terminal");
        terminal.write(b"\x1b[41mHello\x1b[42m World\x1b[0m");
        let state = terminal.snapshot().expect("snapshot");
        let svg = render_svg(&state).expect("render SVG");
        let svg_str = String::from_utf8(svg).expect("valid UTF-8 SVG");
        assert_eq!(svg_str.matches("<text ").count(), 1);
        assert_eq!(svg_str.matches("<tspan x=").count(), "Hello World".len());
        assert!(svg_str.contains("width=\"60\""));
        assert!(svg_str.contains("width=\"72\""));

        let png = render_png(&state).expect("render PNG");
        assert!(!png.is_empty());
    }
}
