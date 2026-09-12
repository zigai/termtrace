//! Focused semantic rendering quality corpus.

use crate::{render_png, render_png_with_profile, render_svg, render_video_rgba};
use resvg::tiny_skia::Pixmap;
use std::{env, fs, path::Path, str};
use termtrace_model::artifact::save::SemanticRasterProfile;
use termtrace_model::terminal::state::{CursorStyle, SemanticState};
use termtrace_terminal::GhosttyVt;

fn quality_corpus() -> SemanticState {
    let mut terminal = GhosttyVt::new(48, 8, 0).expect("create quality corpus terminal");
    terminal.write(
        concat!(
            "AgÉgj ",
            "\x1b[1mbold\x1b[0m ",
            "\x1b[3mitalic\x1b[0m ",
            "\x1b[1;3mbold-italic\x1b[0m\r\n",
            "   ──││██\r\n",
            "\x1b[58;2;255;0;255m",
            "\x1b[4:1m1 \x1b[4:2m2 \x1b[4:3m3 \x1b[4:4m4 \x1b[4:5m5\x1b[0m\r\n",
            "\x1b[9mstrike\x1b[0m \x1b[53moverline\x1b[0m\r\n",
            "narrow A wide 界 tail B\r\n",
            "cursor A",
        )
        .as_bytes(),
    );
    let mut state = terminal.snapshot().expect("snapshot quality corpus");
    state.cursor.visible = true;
    state.cursor.in_viewport = true;
    state.cursor.column = Some(7);
    state.cursor.row = Some(5);
    state.cursor.wide_tail = Some(false);

    return state;
}

fn assert_decoration_geometry(svg: &str, pixmap: &Pixmap) {
    assert!(svg.contains("data-truth-layer=\"semantic_rendering\""));
    assert!(svg.contains("data-renderer="));

    // Each style occupies its own cell in row 2. Measure magenta coverage,
    // excluding the gray glyphs, rather than the SVG decoration syntax.
    let ink = |style: u32, x: u32, y: u32| {
        let pixel = pixmap
            .pixel(style * 24 + x, 2 * 24 + y)
            .expect("decoration cell is inside the rendered viewport");

        return f64::from(pixel.red().min(pixel.blue()).saturating_sub(pixel.green()));
    };

    let columns: Vec<Vec<f64>> = (0..5)
        .map(|style| {
            return (1..11)
                .map(|x| return (19..24).map(|y| return ink(style, x, y)).sum())
                .collect();
        })
        .collect();

    let single = &columns[0];
    let double = &columns[1];
    let dotted = &columns[3];
    let dashed = &columns[4];

    assert!(single.iter().all(|coverage| return *coverage > 100.0));
    assert!(
        double.iter().sum::<f64>() > single.iter().sum::<f64>() * 1.5,
        "double underline must contain two strokes rather than one"
    );

    let upper_ink = |style| {
        return (1..11)
            .flat_map(|x| return (19..22).map(move |y| return ink(style, x, y)))
            .sum::<f64>();
    };

    assert!(
        upper_ink(1) > upper_ink(0) + 500.0,
        "double underline must have a separate upper stroke"
    );

    let curly_centers: Vec<_> = (1..11)
        .map(|x| {
            let weight: f64 = (19..24).map(|y| return ink(2, x, y)).sum();
            assert!(weight > 50.0, "curly underline must remain continuous");

            return (19..24)
                .map(|y| return f64::from(y) * ink(2, x, y))
                .sum::<f64>()
                / weight;
        })
        .collect();

    let low = curly_centers.iter().copied().fold(f64::INFINITY, f64::min);
    let high = curly_centers
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    assert!(high - low > 0.2, "curly underline must rise and fall");

    let runs = |coverage: &[f64]| {
        let low = coverage.iter().copied().fold(f64::INFINITY, f64::min);
        let high = coverage.iter().copied().fold(f64::NEG_INFINITY, f64::max);

        assert!(
            high - low > 100.0,
            "pattern must have contrasting strokes and gaps"
        );

        // Antialiasing leaves partial ink in gaps; compare against the stroke coverage.
        let threshold = f64::midpoint(low, high);
        let painted: Vec<_> = coverage
            .iter()
            .map(|value| return *value > threshold)
            .collect();

        return painted
            .windows(2)
            .filter(|pair| return pair[0] != pair[1])
            .count();
    };

    assert!(
        runs(dotted) > runs(dashed),
        "dotted underline must repeat more frequently than dashed underline"
    );
}

fn assert_cursor_geometry(pixmap: &Pixmap, hidden_pixmap: &Pixmap, style: CursorStyle) {
    let cell_x = 7 * 12;
    let cell_y = 5 * 24;

    match style {
        CursorStyle::Bar => {
            assert_ne!(
                pixmap.pixel(cell_x, cell_y + 12),
                hidden_pixmap.pixel(cell_x, cell_y + 12),
                "bar cursor should modify left edge"
            );
            assert_eq!(
                pixmap.pixel(cell_x + 6, cell_y + 12),
                hidden_pixmap.pixel(cell_x + 6, cell_y + 12),
                "bar cursor should leave middle unchanged"
            );
        }
        CursorStyle::Underline => {
            assert_ne!(
                pixmap.pixel(cell_x + 6, cell_y + 23),
                hidden_pixmap.pixel(cell_x + 6, cell_y + 23),
                "underline cursor should modify bottom edge"
            );
            assert_eq!(
                pixmap.pixel(cell_x + 6, cell_y + 12),
                hidden_pixmap.pixel(cell_x + 6, cell_y + 12),
                "underline cursor should leave middle unchanged"
            );
        }
        CursorStyle::Block => {
            assert_ne!(
                pixmap.pixel(cell_x + 6, cell_y + 12),
                hidden_pixmap.pixel(cell_x + 6, cell_y + 12),
                "block cursor should modify cell center"
            );
        }
        CursorStyle::HollowBlock => {
            assert_ne!(
                pixmap.pixel(cell_x, cell_y),
                hidden_pixmap.pixel(cell_x, cell_y),
                "hollow block cursor should modify border"
            );
            assert_eq!(
                pixmap.pixel(cell_x + 6, cell_y + 6),
                hidden_pixmap.pixel(cell_x + 6, cell_y + 6),
                "hollow block cursor should leave interior unfilled"
            );
        }
    }
}

#[test]
fn quality_corpus_renders_every_cursor_style() {
    let output = env::var_os("TERMTRACE_QUALITY_REFERENCE_DIR");
    let mut hidden_state = quality_corpus();
    hidden_state.cursor.visible = false;
    let hidden_png = render_png(&hidden_state).expect("render hidden cursor PNG");
    let hidden_pixmap = Pixmap::decode_png(&hidden_png).expect("decode hidden cursor PNG");

    for (name, style) in [
        ("bar", CursorStyle::Bar),
        ("block", CursorStyle::Block),
        ("underline", CursorStyle::Underline),
        ("hollow-block", CursorStyle::HollowBlock),
    ] {
        let mut state = quality_corpus();
        state.cursor.style = style;
        let svg = render_svg(&state).expect("render quality corpus SVG");
        let png = render_png(&state).expect("render quality corpus PNG");
        let svg_text = str::from_utf8(&svg).expect("quality corpus SVG is UTF-8");
        let pixmap = Pixmap::decode_png(&png).expect("decode quality corpus PNG");
        assert_decoration_geometry(svg_text, &pixmap);
        assert_cursor_geometry(&pixmap, &hidden_pixmap, style);

        if let Some(directory) = output.as_deref() {
            fs::write(Path::new(directory).join(format!("{name}.svg")), svg)
                .expect("write quality corpus SVG reference");

            fs::write(Path::new(directory).join(format!("{name}.png")), &png)
                .expect("write quality corpus PNG reference");
        }
    }
}

#[test]
fn cursor_visibility_and_wide_tail_are_composed_from_terminal_state() {
    let mut hidden = quality_corpus();
    hidden.cursor.visible = false;
    let hidden_svg = render_svg(&hidden).expect("render hidden cursor SVG");
    let mut out_of_viewport = quality_corpus();
    out_of_viewport.cursor.in_viewport = false;
    let out_of_viewport_svg =
        render_svg(&out_of_viewport).expect("render out-of-viewport cursor SVG");
    assert_eq!(out_of_viewport_svg, hidden_svg);

    let mut wide_tail = quality_corpus();
    let (row, lead_column) = wide_tail
        .grid
        .iter()
        .flat_map(|row| {
            return row.cells.iter().map(move |cell| return (row.row, cell));
        })
        .find_map(|(row, cell)| {
            return (cell.grapheme == "界").then_some((row, cell.column));
        })
        .expect("quality corpus contains wide grapheme");

    wide_tail.cursor.style = CursorStyle::Block;
    wide_tail.cursor.row = Some(row);
    wide_tail.cursor.column = lead_column.checked_add(1);
    wide_tail.cursor.wide_tail = Some(true);
    let wide_svg = render_svg(&wide_tail).expect("render wide-tail cursor SVG");
    let wide_svg = str::from_utf8(&wide_svg).expect("wide-tail SVG is UTF-8");
    assert!(wide_svg.contains("界"));

    let wide_png = render_png(&wide_tail).expect("render wide-tail cursor PNG");
    let wide_pixmap = Pixmap::decode_png(&wide_png).expect("decode wide-tail PNG");
    let mut hidden_wide = wide_tail;
    hidden_wide.cursor.visible = false;
    let hidden_wide_png = render_png(&hidden_wide).expect("render hidden wide PNG");
    let hidden_wide_pixmap = Pixmap::decode_png(&hidden_wide_png).expect("decode hidden wide PNG");
    let x_lead = u32::from(lead_column) * 12;
    let x_tail = x_lead + 12;
    let y = u32::from(row) * 24;

    assert_ne!(
        wide_pixmap.pixel(x_lead + 6, y + 12),
        hidden_wide_pixmap.pixel(x_lead + 6, y + 12),
        "wide-tail block cursor should modify lead cell"
    );
    assert_ne!(
        wide_pixmap.pixel(x_tail + 6, y + 12),
        hidden_wide_pixmap.pixel(x_tail + 6, y + 12),
        "wide-tail block cursor should modify tail cell"
    );
}

#[test]
fn quality_corpus_is_deterministic_under_each_raster_profile() {
    let state = quality_corpus();
    let expected_width = u32::from(state.cols) * 12;
    let expected_height = u32::from(state.rows) * 24;
    let expected_rgba_len =
        usize::try_from(expected_width * expected_height * 4).expect("RGBA size fits usize");

    let mut direct_hash = None;
    let mut supersampled_hash = None;

    for profile in [
        SemanticRasterProfile::Direct,
        SemanticRasterProfile::Supersampled2x,
    ] {
        let first_png =
            render_png_with_profile(&state, profile).expect("render first profiled PNG");

        let second_png =
            render_png_with_profile(&state, profile).expect("render second profiled PNG");
        assert_eq!(first_png, second_png);

        let pixmap = Pixmap::decode_png(&first_png).expect("decode profiled PNG");
        assert_eq!(pixmap.width(), expected_width);
        assert_eq!(pixmap.height(), expected_height);

        let first_rgba = render_video_rgba(&state, state.cols, state.rows, profile)
            .expect("render first profiled RGBA frame");

        let second_rgba = render_video_rgba(&state, state.cols, state.rows, profile)
            .expect("render second profiled RGBA frame");
        assert_eq!(first_rgba, second_rgba);
        assert_eq!(first_rgba.len(), expected_rgba_len);
        assert!(first_rgba
            .chunks_exact(4)
            .all(|pixel| return pixel[3] == u8::MAX));

        match profile {
            SemanticRasterProfile::Direct => direct_hash = Some(blake3::hash(&first_rgba)),
            SemanticRasterProfile::Supersampled2x => {
                supersampled_hash = Some(blake3::hash(&first_rgba));
            }
        }
    }

    assert_ne!(
        direct_hash.expect("direct hash recorded"),
        supersampled_hash.expect("supersampled hash recorded")
    );
    assert_eq!(
        render_png(&state).expect("render screenshot-quality PNG"),
        render_png_with_profile(&state, SemanticRasterProfile::Supersampled2x)
            .expect("render explicit supersampled PNG")
    );
}
