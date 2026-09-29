//! Deterministic PNG and opaque RGBA rasterization.

use crate::error::{render_error, Result};
use crate::font::{is_variation_selector, semantic_font_set};
use crate::semantic_svg::{
    font_geometry, is_terminal_graphic, render_svg_scene, resolve_color, select_font_role,
    SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH,
};
use resvg::tiny_skia::{
    Color, FillRule, FilterQuality, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke,
    Transform,
};
use resvg::usvg::{Options, Tree};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    mem,
    sync::{Arc, LazyLock},
};
use termtrace_model::artifact::save::{SemanticFontRole, SemanticRasterProfile};
use termtrace_model::terminal::state::{
    CursorStyle, Rgb, SemanticState, WideKind, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS,
};
use termtrace_terminal::supported_geometry_cell_count;

thread_local! {
    static INTERMEDIATE_SURFACE: RefCell<Option<Pixmap>> = const { RefCell::new(None) };
    static GLYPH_CACHE: RefCell<HashMap<(u8, u16, u16), Option<Path>>> = RefCell::new(HashMap::new());
}

struct GlyphPathBuilder {
    builder: PathBuilder,
    scale: f32,
}

impl ttf_parser::OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x * self.scale, -y * self.scale);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x * self.scale, -y * self.scale);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.builder.quad_to(
            x1 * self.scale,
            -y1 * self.scale,
            x * self.scale,
            -y * self.scale,
        );
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            x1 * self.scale,
            -y1 * self.scale,
            x2 * self.scale,
            -y2 * self.scale,
            x * self.scale,
            -y * self.scale,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

struct ParsedFaces {
    faces: [Option<ttf_parser::Face<'static>>; 5],
}

impl ParsedFaces {
    fn new() -> Self {
        let Ok(font_set) = semantic_font_set() else {
            return Self {
                faces: [const { None }; 5],
            };
        };

        return Self {
            faces: [
                ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::Regular), 0).ok(),
                ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::Bold), 0).ok(),
                ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::Italic), 0).ok(),
                ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::BoldItalic), 0).ok(),
                ttf_parser::Face::parse(font_set.face_bytes(SemanticFontRole::SymbolsFallback), 0)
                    .ok(),
            ],
        };
    }

    fn face(&self, role: SemanticFontRole) -> Option<&ttf_parser::Face<'static>> {
        let index = match role {
            SemanticFontRole::Regular => 0,
            SemanticFontRole::Bold => 1,
            SemanticFontRole::Italic => 2,
            SemanticFontRole::BoldItalic => 3,
            SemanticFontRole::SymbolsFallback => 4,
        };

        return self.faces.get(index).and_then(Option::as_ref);
    }
}

fn get_glyph_path(
    faces: &ParsedFaces,
    role: SemanticFontRole,
    ch: char,
    font_size: u16,
) -> Option<Path> {
    let face = faces.face(role)?;
    let glyph_id = face.glyph_index(ch)?;
    let key = (role as u8, glyph_id.0, font_size);

    return GLYPH_CACHE.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(path) = map.get(&key) {
            return path.clone();
        }

        let scale = (f32::from(font_size)) / (f32::from(face.units_per_em()));
        let mut builder = GlyphPathBuilder {
            builder: PathBuilder::new(),
            scale,
        };
        let path = if face.outline_glyph(glyph_id, &mut builder).is_some() {
            builder.builder.finish()
        } else {
            None
        };
        map.insert(key, path.clone());

        return path;
    });
}

/// Scale a cell or font measure to whole pixels, rejecting results the cast would saturate.
fn scaled_pixels(base: u16, scale: f32) -> Result<u16> {
    let scaled = (f32::from(base) * scale).round();
    if !scaled.is_finite() || scaled < 0.0 || scaled > f32::from(u16::MAX) {
        return Err(render_error(format!(
            "scaled semantic geometry {scaled} does not fit 16-bit pixels"
        )));
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked above to be finite and within `0..=u16::MAX`, and `round` makes it \
                  whole, so the cast is exact; std has no checked float-to-integer conversion"
    )]
    let pixels = scaled as u16;

    return Ok(pixels);
}

fn render_grid_direct(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
    scale: f32,
    opaque_background: Option<Rgb>,
) -> Result<Pixmap> {
    static FACES: LazyLock<ParsedFaces> = LazyLock::new(ParsedFaces::new);
    let faces = &*FACES;
    let font_set = semantic_font_set()?;
    let cell_w = u32::from(scaled_pixels(SEMANTIC_CELL_WIDTH, scale)?);
    let cell_h = u32::from(scaled_pixels(SEMANTIC_CELL_HEIGHT, scale)?);
    let width = u32::from(canvas_cols).saturating_mul(cell_w);
    let height = u32::from(canvas_rows).saturating_mul(cell_h);
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;

    let bg = opaque_background.unwrap_or(state.colors.background);
    pixmap.fill(Color::from_rgba8(bg.r, bg.g, bg.b, u8::MAX));

    for row in &state.grid {
        let row_y = (f32::from(row.row) * f32::from(SEMANTIC_CELL_HEIGHT) * scale).round();

        // 1. Cell Backgrounds
        for cell in &row.cells {
            let span = match cell.wide_kind {
                WideKind::Narrow => f32::from(SEMANTIC_CELL_WIDTH) * scale,
                WideKind::Wide => f32::from(SEMANTIC_CELL_WIDTH) * 2.0 * scale,
                WideKind::SpacerTail | WideKind::SpacerHead => continue,
            };

            let cell_x = (f32::from(cell.column) * f32::from(SEMANTIC_CELL_WIDTH) * scale).round();

            let mut bg_color = cell.resolved_background.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.background,
                    state.colors.background,
                    &state.colors.palette,
                );
            });

            let mut fg_color = cell.resolved_foreground.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.foreground,
                    state.colors.foreground,
                    &state.colors.palette,
                );
            });

            if cell.style.inverse {
                mem::swap(&mut fg_color, &mut bg_color);
            }

            if bg_color != state.colors.background || cell.style.inverse {
                let mut bg_paint = Paint::default();
                bg_paint.set_color_rgba8(bg_color.r, bg_color.g, bg_color.b, u8::MAX);

                if let Some(rect) =
                    Rect::from_xywh(cell_x, row_y, span, f32::from(SEMANTIC_CELL_HEIGHT) * scale)
                {
                    pixmap.fill_rect(rect, &bg_paint, Transform::identity(), None);
                }
            }
        }

        // 2. Text Glyphs
        for cell in &row.cells {
            if cell.style.invisible || cell.grapheme.is_empty() {
                continue;
            }

            if matches!(cell.wide_kind, WideKind::SpacerTail | WideKind::SpacerHead) {
                continue;
            }

            let cell_x = (f32::from(cell.column) * f32::from(SEMANTIC_CELL_WIDTH) * scale).round();

            let mut fg_color = cell.resolved_foreground.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.foreground,
                    state.colors.foreground,
                    &state.colors.palette,
                );
            });

            let mut bg_color = cell.resolved_background.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.background,
                    state.colors.background,
                    &state.colors.palette,
                );
            });

            if cell.style.inverse {
                mem::swap(&mut fg_color, &mut bg_color);
            }

            let role = select_font_role(font_set, &cell.style, &cell.grapheme);
            let (base_size, base_line) = font_geometry(role);
            let size = scaled_pixels(base_size, scale)?;
            let baseline_y = row_y + (f32::from(base_line) * scale).round();

            let opacity = if cell.style.faint { 153 } else { 255 };
            let mut text_paint = Paint::default();
            text_paint.set_color_rgba8(fg_color.r, fg_color.g, fg_color.b, opacity);
            text_paint.anti_alias = !is_terminal_graphic(&cell.grapheme);

            for ch in cell.grapheme.chars() {
                if is_variation_selector(ch) {
                    continue;
                }

                if let Some(path) = get_glyph_path(faces, role, ch, size) {
                    pixmap.fill_path(
                        &path,
                        &text_paint,
                        FillRule::Winding,
                        Transform::from_translate(cell_x, baseline_y),
                        None,
                    );
                }
            }
        }

        // 3. Decorations
        for cell in &row.cells {
            if matches!(cell.wide_kind, WideKind::SpacerTail | WideKind::SpacerHead) {
                continue;
            }

            let cell_x = (f32::from(cell.column) * f32::from(SEMANTIC_CELL_WIDTH) * scale).round();
            let span = match cell.wide_kind {
                WideKind::Narrow => f32::from(SEMANTIC_CELL_WIDTH) * scale,
                WideKind::Wide => f32::from(SEMANTIC_CELL_WIDTH) * 2.0 * scale,
                _ => continue,
            };

            let mut fg_color = cell.resolved_foreground.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.foreground,
                    state.colors.foreground,
                    &state.colors.palette,
                );
            });

            let mut bg_color = cell.resolved_background.unwrap_or_else(|| {
                return resolve_color(
                    cell.style.background,
                    state.colors.background,
                    &state.colors.palette,
                );
            });

            if cell.style.inverse {
                mem::swap(&mut fg_color, &mut bg_color);
            }

            let underline_color =
                resolve_color(cell.style.underline_color, fg_color, &state.colors.palette);

            let opacity = if cell.style.faint { 153 } else { 255 };

            let mut u_paint = Paint::default();
            u_paint.set_color_rgba8(
                underline_color.r,
                underline_color.g,
                underline_color.b,
                opacity,
            );

            match cell.style.underline {
                1 => {
                    let y = row_y + (23.0 * scale).round();

                    if let Some(rect) =
                        Rect::from_xywh(cell_x, y, span, (1.0 * scale).round().max(1.0))
                    {
                        pixmap.fill_rect(rect, &u_paint, Transform::identity(), None);
                    }
                }
                2 => {
                    let y1 = row_y + (21.0 * scale).round();
                    let y2 = row_y + (23.0 * scale).round();
                    let h = (1.0 * scale).round().max(1.0);

                    if let Some(rect) = Rect::from_xywh(cell_x, y1, span, h) {
                        pixmap.fill_rect(rect, &u_paint, Transform::identity(), None);
                    }

                    if let Some(rect) = Rect::from_xywh(cell_x, y2, span, h) {
                        pixmap.fill_rect(rect, &u_paint, Transform::identity(), None);
                    }
                }
                3 => {
                    let y = row_y + (22.0 * scale).round();
                    let upper_y = y - 1.0 * scale;
                    let lower_y = y + 1.0 * scale;
                    let mut pb = PathBuilder::new();
                    pb.move_to(cell_x, y);

                    let step = (3.0 * scale).max(1.0);
                    let mut start = cell_x;

                    while start < cell_x + span {
                        pb.quad_to(start + 0.75 * scale, upper_y, start + 1.5 * scale, y);
                        pb.quad_to(start + 2.25 * scale, lower_y, start + 3.0 * scale, y);
                        start += step;
                    }

                    if let Some(path) = pb.finish() {
                        let stroke = Stroke {
                            width: (1.0 * scale).round().max(1.0),
                            ..Stroke::default()
                        };
                        pixmap.stroke_path(&path, &u_paint, &stroke, Transform::identity(), None);
                    }
                }
                4 => {
                    let y = row_y + (23.0 * scale).round();
                    let h = (1.0 * scale).round().max(1.0);
                    let mut start = cell_x;

                    while start < cell_x + span {
                        if let Some(rect) = Rect::from_xywh(start, y, 1.0 * scale, h) {
                            pixmap.fill_rect(rect, &u_paint, Transform::identity(), None);
                        }

                        start += 3.0 * scale;
                    }
                }
                5 => {
                    let y = row_y + (23.0 * scale).round();
                    let h = (1.0 * scale).round().max(1.0);
                    let mut start = cell_x;

                    while start < cell_x + span {
                        let dash_w = (4.0 * scale).min(cell_x + span - start);
                        if let Some(rect) = Rect::from_xywh(start, y, dash_w, h) {
                            pixmap.fill_rect(rect, &u_paint, Transform::identity(), None);
                        }

                        start += 6.0 * scale;
                    }
                }
                _ => {}
            }

            if cell.style.strikethrough {
                let mut s_paint = Paint::default();
                s_paint.set_color_rgba8(fg_color.r, fg_color.g, fg_color.b, opacity);

                let y = row_y + (14.0 * scale).round();

                if let Some(rect) = Rect::from_xywh(cell_x, y, span, (1.0 * scale).round().max(1.0))
                {
                    pixmap.fill_rect(rect, &s_paint, Transform::identity(), None);
                }
            }

            if cell.style.overline {
                let mut o_paint = Paint::default();
                o_paint.set_color_rgba8(fg_color.r, fg_color.g, fg_color.b, opacity);

                let y = row_y + (1.0 * scale).round();

                if let Some(rect) = Rect::from_xywh(cell_x, y, span, (1.0 * scale).round().max(1.0))
                {
                    pixmap.fill_rect(rect, &o_paint, Transform::identity(), None);
                }
            }
        }
    }

    // 4. Cursor
    if state.cursor.visible
        && state.cursor.in_viewport
        && let (Some(col), Some(row)) = (state.cursor.column, state.cursor.row)
    {
        let is_wide_tail = state.cursor.wide_tail == Some(true);
        let col = if is_wide_tail { col.saturating_sub(1) } else { col };
        let x = (f32::from(col) * f32::from(SEMANTIC_CELL_WIDTH) * scale).round();
        let y = (f32::from(row) * f32::from(SEMANTIC_CELL_HEIGHT) * scale).round();
        let cursor_color = state.colors.cursor.unwrap_or(state.colors.foreground);
        let mut paint = Paint::default();
        paint.set_color_rgba8(cursor_color.r, cursor_color.g, cursor_color.b, u8::MAX);

        match state.cursor.style {
            CursorStyle::Bar => {
                let w = (2.0 * scale).round().max(1.0);
                let h = f32::from(SEMANTIC_CELL_HEIGHT) * scale;

                if let Some(rect) = Rect::from_xywh(x, y, w, h) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            CursorStyle::Underline => {
                let w = f32::from(SEMANTIC_CELL_WIDTH) * scale;
                let h = (2.0 * scale).round().max(1.0);
                let cur_y = y + (22.0 * scale).round();

                if let Some(rect) = Rect::from_xywh(x, cur_y, w, h) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            CursorStyle::Block => {
                let w = if is_wide_tail {
                    f32::from(SEMANTIC_CELL_WIDTH) * 2.0 * scale
                } else {
                    f32::from(SEMANTIC_CELL_WIDTH) * scale
                };

                let h = f32::from(SEMANTIC_CELL_HEIGHT) * scale;
                if let Some(rect) = Rect::from_xywh(x, y, w, h) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }

                if let Some(row_state) = state.grid.iter().find(|r| return r.row == row)
                    && let Some(cell) = row_state.cells.iter().find(|c| return c.column == col)
                    && !cell.grapheme.is_empty()
                    && !cell.style.invisible
                {
                    let mut glyph_paint = Paint::default();
                    let bg_col = state.colors.background;
                    glyph_paint.set_color_rgba8(bg_col.r, bg_col.g, bg_col.b, u8::MAX);

                    let role = select_font_role(font_set, &cell.style, &cell.grapheme);
                    let (base_size, base_line) = font_geometry(role);
                    let size = scaled_pixels(base_size, scale)?;
                    let baseline_y = y + (f32::from(base_line) * scale).round();

                    for ch in cell.grapheme.chars() {
                        if !is_variation_selector(ch)
                            && let Some(path) = get_glyph_path(faces, role, ch, size)
                        {
                            pixmap.fill_path(
                                &path,
                                &glyph_paint,
                                FillRule::Winding,
                                Transform::from_translate(x, baseline_y),
                                None,
                            );
                        }
                    }
                }
            }
            CursorStyle::HollowBlock => {
                let w = (f32::from(SEMANTIC_CELL_WIDTH) * scale) - 1.0;
                let h = (f32::from(SEMANTIC_CELL_HEIGHT) * scale) - 1.0;

                if let Some(rect) = Rect::from_xywh(x + 0.5, y + 0.5, w, h) {
                    let stroke = Stroke {
                        width: 1.0,
                        ..Stroke::default()
                    };

                    let path = PathBuilder::from_rect(rect);
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                }
            }
        }
    }

    return Ok(pixmap);
}

/// Stable semantic PNG rasterizer identity.
pub const SEMANTIC_PNG_RENDERER: &str = "termtrace-semantic-png-resvg-v4";

/// Rasterize the exact deterministic SVG scene with only the bundled font.
///
/// # Errors
///
/// Returns an error when the semantic geometry is outside the supported bounds,
/// the SVG cannot be parsed, or the PNG cannot be encoded.
pub fn render_png(state: &SemanticState) -> Result<Vec<u8>> {
    return render_png_with_profile(state, SemanticRasterProfile::Supersampled2x);
}

/// Rasterize a semantic state with an explicit deterministic quality profile.
///
/// # Errors
///
/// Returns an error when the semantic geometry is outside the supported bounds,
/// the SVG cannot be parsed, an intermediate surface cannot be allocated, or
/// the PNG cannot be encoded.
pub fn render_png_with_profile(
    state: &SemanticState,
    profile: SemanticRasterProfile,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    write_png(state, profile, &mut bytes)?;

    return Ok(bytes);
}

/// Encode a semantic screenshot directly into a fallible streaming destination.
///
/// # Errors
///
/// Returns rendering, encoding, or destination write errors without materializing the PNG.
pub fn write_png(
    state: &SemanticState,
    profile: SemanticRasterProfile,
    writer: impl Write,
) -> Result<()> {
    validate_geometry(state.cols, state.rows, "semantic state")?;
    let pixmap = render_semantic_pixmap(
        state,
        state.cols,
        state.rows,
        profile,
        Some(state.colors.background),
    )?;

    let mut encoder = png::Encoder::new(writer, pixmap.width(), pixmap.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder
        .write_header()
        .map_err(|error| return render_error(format!("encode semantic PNG: {error}")))?;
    let mut stream = png
        .stream_writer()
        .map_err(|error| return render_error(format!("encode semantic PNG: {error}")))?;
    stream
        .write_all(pixmap.data())
        .map_err(|error| return render_error(format!("write semantic PNG: {error}")))?;
    stream
        .finish()
        .map_err(|error| return render_error(format!("finish semantic PNG: {error}")))?;

    return png
        .finish()
        .map_err(|error| return render_error(format!("finish semantic PNG: {error}")));
}

fn render_semantic_pixmap(
    state: &SemanticState,
    canvas_cols: u16,
    canvas_rows: u16,
    profile: SemanticRasterProfile,
    opaque_background: Option<Rgb>,
) -> Result<Pixmap> {
    match profile {
        SemanticRasterProfile::Direct => {
            return render_grid_direct(state, canvas_cols, canvas_rows, 1.0, opaque_background);
        }
        SemanticRasterProfile::Supersampled2x => {
            return rasterize_svg_pixmap(
                &render_svg_scene(state, canvas_cols, canvas_rows, false)?,
                profile,
                opaque_background,
            );
        }
    }
}

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
    profile: SemanticRasterProfile,
) -> Result<Vec<u8>> {
    validate_geometry(state.cols, state.rows, "semantic state")?;

    validate_canvas_geometry(canvas_cols, canvas_rows)?;
    if canvas_cols < state.cols || canvas_rows < state.rows {
        return Err(render_error(format!(
            "semantic video canvas {canvas_cols}x{canvas_rows} is smaller than state {}x{}",
            state.cols, state.rows
        )));
    }

    let pixmap = render_semantic_pixmap(
        state,
        canvas_cols,
        canvas_rows,
        profile,
        Some(state.colors.background),
    )?;

    return Ok(pixmap.take());
}

pub(super) fn rasterize_svg(svg: &[u8]) -> Result<Vec<u8>> {
    return rasterize_svg_pixmap(svg, SemanticRasterProfile::Direct, None)?
        .encode_png()
        .map_err(|error| return render_error(format!("encode semantic PNG: {error}")));
}

fn rasterize_svg_pixmap(
    svg: &[u8],
    profile: SemanticRasterProfile,
    opaque_background: Option<Rgb>,
) -> Result<Pixmap> {
    let options = Options {
        fontdb: Arc::clone(semantic_font_set()?.database()),
        ..Options::default()
    };
    let tree = Tree::from_data(svg, &options)
        .map_err(|error| return render_error(format!("parse deterministic SVG: {error}")))?;
    let size = tree.size().to_int_size();

    if profile == SemanticRasterProfile::Direct {
        let mut pixmap = Pixmap::new(size.width(), size.height())
            .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;

        if let Some(background) = opaque_background {
            pixmap.fill(Color::from_rgba8(
                background.r,
                background.g,
                background.b,
                u8::MAX,
            ));
        }

        resvg::render(&tree, Transform::default(), &mut pixmap.as_mut());

        return Ok(pixmap);
    }

    let width = size
        .width()
        .checked_mul(2)
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;
    let height = size
        .height()
        .checked_mul(2)
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;

    let mut intermediate = INTERMEDIATE_SURFACE
        .with(|cell| {
            let mut slot = cell.borrow_mut();
            if let Some(surface) = slot.take()
                && surface.width() == width
                && surface.height() == height
            {
                return Some(surface);
            }
            return Pixmap::new(width, height);
        })
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;

    if let Some(background) = opaque_background {
        intermediate.fill(Color::from_rgba8(
            background.r,
            background.g,
            background.b,
            u8::MAX,
        ));
    } else {
        intermediate.fill(Color::TRANSPARENT);
    }

    resvg::render(
        &tree,
        Transform::from_scale(2.0, 2.0),
        &mut intermediate.as_mut(),
    );

    let mut target = Pixmap::new(size.width(), size.height())
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;

    if let Some(background) = opaque_background {
        target.fill(Color::from_rgba8(
            background.r,
            background.g,
            background.b,
            u8::MAX,
        ));
    }

    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    target.draw_pixmap(
        0,
        0,
        intermediate.as_ref(),
        &paint,
        Transform::from_scale(0.5, 0.5),
        None,
    );

    INTERMEDIATE_SURFACE.with(|cell| {
        *cell.borrow_mut() = Some(intermediate);
    });

    return Ok(target);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_svg::render_svg_scene;
    use resvg::tiny_skia::PremultipliedColorU8;
    use termtrace_model::terminal::state::CursorStyle;
    use termtrace_terminal::GhosttyVt;

    fn rasterize_state(state: &SemanticState) -> Pixmap {
        let svg =
            render_svg_scene(state, state.cols, state.rows, false).expect("render raster test SVG");

        return rasterize_svg_pixmap(
            &svg,
            SemanticRasterProfile::Direct,
            Some(state.colors.background),
        )
        .expect("rasterize test SVG");
    }

    fn is_background(pixel: PremultipliedColorU8, background: Rgb) -> bool {
        return pixel.red() == background.r
            && pixel.green() == background.g
            && pixel.blue() == background.b;
    }

    #[test]
    fn terminal_graphics_cross_their_authoritative_cell_boundaries() {
        let mut terminal = GhosttyVt::new(4, 4, 0).expect("create graphics terminal");
        terminal.write("──\x1b[2;1H│\x1b[3;1H│\x1b[4;1H██".as_bytes());
        let mut state = terminal.snapshot().expect("snapshot graphics terminal");
        state.cursor.visible = false;
        let pixmap = rasterize_state(&state);
        let background = state.colors.background;

        assert!((0..24).any(|y| {
            return pixmap
                .pixel(12, y)
                .is_some_and(|pixel| return !is_background(pixel, background));
        }));
        assert!((0..12).any(|x| {
            let above = pixmap
                .pixel(x, 47)
                .is_some_and(|pixel| return !is_background(pixel, background));
            let below = pixmap
                .pixel(x, 48)
                .is_some_and(|pixel| return !is_background(pixel, background));
            return above && below;
        }));
        assert!((72..96).all(|y| {
            return pixmap
                .pixel(12, y)
                .is_some_and(|pixel| return !is_background(pixel, background));
        }));
    }

    #[test]
    fn symbols_fallback_stays_inside_the_canvas_edges() {
        let mut terminal = GhosttyVt::new(4, 1, 0).expect("create symbols terminal");
        terminal.write(" ".as_bytes());
        let mut state = terminal.snapshot().expect("snapshot symbols terminal");
        state.cursor.visible = false;
        let pixmap = rasterize_state(&state);
        let background = state.colors.background;
        let painted_rows: Vec<u32> = (0..pixmap.height())
            .filter(|y| {
                return (0..pixmap.width()).any(|x| {
                    return pixmap
                        .pixel(x, *y)
                        .is_some_and(|pixel| return !is_background(pixel, background));
                });
            })
            .collect();

        let first = painted_rows.first().copied().expect("icons paint pixels");
        let last = painted_rows.last().copied().expect("icons paint pixels");

        assert!(first > 0);
        assert!(last < pixmap.height().saturating_sub(1));
    }

    #[test]
    fn prompt_and_cli_glyphs_render_cleanly() {
        let mut terminal = GhosttyVt::new(10, 1, 0).expect("create terminal");
        terminal.write("   ❯ ".as_bytes());
        let mut state = terminal.snapshot().expect("snapshot terminal");
        state.cursor.visible = false;
        let pixmap = rasterize_state(&state);
        let background = state.colors.background;

        for (i, glyph) in ["", " ", "", " ", "", " ", "❯", " ", ""]
            .iter()
            .enumerate()
        {
            if *glyph == " " {
                continue;
            }

            let col = u32::try_from(i).expect("glyph column fits u32");
            let has_pixels = (col * 12..(col + 1) * 12).any(|x| {
                return (0..24).any(|y| {
                    return pixmap
                        .pixel(x, y)
                        .is_some_and(|p| return !is_background(p, background));
                });
            });
            assert!(has_pixels, "glyph {glyph} must paint non-background pixels");
        }
    }

    #[test]
    fn truecolor_and_ansi256_render_exact_palette_colors() {
        let mut terminal = GhosttyVt::new(8, 2, 0).expect("create terminal");
        // TrueColor RGB (123, 45, 67) background and ANSI-256 color index 196 (red: 255, 0, 0) background
        terminal.write(b"\x1b[48;2;123;45;67m  \x1b[48;5;196m  ");
        let mut state = terminal.snapshot().expect("snapshot");
        state.cursor.visible = false;
        let pixmap = rasterize_state(&state);

        // Cells 0 and 1 have background (123, 45, 67)
        for x in 0..24 {
            for y in 0..24 {
                let pixel = pixmap.pixel(x, y).expect("pixel exists");
                assert_eq!(pixel.red(), 123);
                assert_eq!(pixel.green(), 45);
                assert_eq!(pixel.blue(), 67);
            }
        }

        // Cells 2 and 3 have ANSI-256 index 196 background (255, 0, 0)
        for x in 24..48 {
            for y in 0..24 {
                let pixel = pixmap.pixel(x, y).expect("pixel exists");
                assert_eq!(pixel.red(), 255);
                assert_eq!(pixel.green(), 0);
                assert_eq!(pixel.blue(), 0);
            }
        }
    }

    #[test]
    fn hollow_and_block_cursors_preserve_terminal_cell_composition() {
        let mut terminal = GhosttyVt::new(1, 1, 0).expect("create cursor terminal");
        terminal.write(b"A");
        let mut state = terminal.snapshot().expect("snapshot cursor terminal");
        state.cursor.visible = true;
        state.cursor.in_viewport = true;
        state.cursor.column = Some(0);
        state.cursor.row = Some(0);
        state.cursor.wide_tail = Some(false);
        state.cursor.style = CursorStyle::HollowBlock;
        let hollow = rasterize_state(&state);

        state.cursor.visible = false;
        let hidden = rasterize_state(&state);

        for y in 2..22 {
            for x in 2..10 {
                assert_eq!(hollow.pixel(x, y), hidden.pixel(x, y));
            }
        }

        state.cursor.visible = true;
        state.cursor.style = CursorStyle::Block;
        let block = rasterize_state(&state);
        let cursor = state.colors.cursor.unwrap_or(state.colors.foreground);
        let background = state.colors.background;

        assert!((0..block.height()).any(|y| {
            return (0..block.width()).any(|x| {
                return block.pixel(x, y).is_some_and(|pixel| {
                    return pixel.red() == cursor.r
                        && pixel.green() == cursor.g
                        && pixel.blue() == cursor.b;
                });
            });
        }));
        assert!((0..block.height()).any(|y| {
            return (0..block.width()).any(|x| {
                return block
                    .pixel(x, y)
                    .is_some_and(|pixel| return is_background(pixel, background));
            });
        }));
    }
}
