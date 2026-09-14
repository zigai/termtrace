//! Deterministic PNG and opaque RGBA rasterization.

use crate::error::{render_error, Result};
use crate::font::semantic_font_set;
use crate::semantic_svg::render_svg_scene;
use resvg::tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};
use resvg::usvg::{Options, Tree};
use std::{io::Write, sync::Arc};
use termtrace_model::artifact::save::SemanticRasterProfile;
use termtrace_model::terminal::state::{
    Rgb, SemanticState, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS,
};
use termtrace_terminal::supported_geometry_cell_count;

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
    let svg = render_svg_scene(state, state.cols, state.rows, false)?;
    let pixmap = rasterize_svg_pixmap(&svg, profile, Some(state.colors.background))?;
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

    let pixmap = rasterize_svg_pixmap(
        &render_svg_scene(state, canvas_cols, canvas_rows, false)?,
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
    let mut intermediate = Pixmap::new(width, height)
        .ok_or_else(|| return render_error("allocate semantic PNG surface"))?;
    if let Some(background) = opaque_background {
        intermediate.fill(Color::from_rgba8(
            background.r,
            background.g,
            background.b,
            u8::MAX,
        ));
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
        for (i, glyph) in ["", " ", "", " ", "", " ", "❯", " ", ""].iter().enumerate() {
            if *glyph == " " {
                continue;
            }
            let col = i as u32;
            let has_pixels = (col * 12..(col + 1) * 12).any(|x| {
                (0..24).any(|y| {
                    pixmap.pixel(x, y).is_some_and(|p| !is_background(p, background))
                })
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
