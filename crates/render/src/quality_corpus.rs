//! Focused semantic rendering quality corpus.

use std::{env, fs, path::Path, str};

use resvg::tiny_skia::Pixmap;
use termtrace_model::artifact::save::SemanticRasterProfile;
use termtrace_model::terminal::state::{CursorStyle, SemanticState};
use termtrace_terminal::GhosttyVt;

use crate::{render_png, render_png_with_profile, render_svg, render_video_rgba};

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

fn assert_decoration_geometry(svg: &str) {
    assert!(svg.contains(
        "<line x1=\"0\" y1=\"71\" x2=\"12\" y2=\"71\" stroke=\"#ff00ff\" stroke-width=\"1\" opacity=\"1\"/>"
    ));
    assert!(svg.contains(
        "<line x1=\"24\" y1=\"69\" x2=\"36\" y2=\"69\" stroke=\"#ff00ff\" stroke-width=\"1\" opacity=\"1\"/>"
    ));
    assert!(svg.contains(
        "<line x1=\"24\" y1=\"71\" x2=\"36\" y2=\"71\" stroke=\"#ff00ff\" stroke-width=\"1\" opacity=\"1\"/>"
    ));
    assert!(svg.contains("<path d=\"M 48 70 Q 48.75 69 49.5 70 Q 50.25 71 51 70"));
    assert!(svg.contains(
        "<line x1=\"72\" y1=\"71\" x2=\"84\" y2=\"71\" stroke=\"#ff00ff\" stroke-width=\"1\" opacity=\"1\" stroke-dasharray=\"1 2\" stroke-linecap=\"round\"/>"
    ));
    assert!(svg.contains(
        "<line x1=\"96\" y1=\"71\" x2=\"108\" y2=\"71\" stroke=\"#ff00ff\" stroke-width=\"1\" opacity=\"1\" stroke-dasharray=\"4 2\"/>"
    ));
    assert!(svg.contains("<line x1=\"0\" y1=\"86\" x2=\"12\" y2=\"86\""));
    assert!(svg.contains("<line x1=\"84\" y1=\"73\" x2=\"96\" y2=\"73\""));
    assert!(!svg.contains("text-decoration="));
}

fn assert_cursor_geometry(svg: &str, style: CursorStyle) {
    match style {
        CursorStyle::Bar => {
            assert!(svg.contains("<rect x=\"84\" y=\"120\" width=\"2\" height=\"24\""));
        }
        CursorStyle::Underline => {
            assert!(svg.contains("<rect x=\"84\" y=\"142\" width=\"12\" height=\"2\""));
        }
        CursorStyle::Block => {
            let cursor = svg
                .find("<rect x=\"84\" y=\"120\" width=\"12\" height=\"24\"")
                .expect("opaque block cursor geometry");
            let after_cursor = svg
                .get(cursor..)
                .expect("str::find always returns a UTF-8 boundary");
            assert!(!after_cursor.starts_with(
                "<rect x=\"84\" y=\"120\" width=\"12\" height=\"24\" fill=\"#ffffff\" opacity="
            ));
            assert!(after_cursor.contains("<text x=\"84\" y=\"140\" fill=\"#000000\""));
        }
        CursorStyle::HollowBlock => {
            assert!(
                svg.contains(
                    "<rect x=\"84.5\" y=\"120.5\" width=\"11\" height=\"23\" fill=\"none\""
                )
            );
        }
    }
}

#[test]
fn quality_corpus_renders_every_cursor_style() {
    let output = env::var_os("TERMTRACE_QUALITY_REFERENCE_DIR");
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
        assert_decoration_geometry(svg_text);
        assert_cursor_geometry(svg_text, style);
        assert!(!svg.is_empty());
        assert!(!png.is_empty());
        if let Some(directory) = output.as_deref() {
            fs::write(Path::new(directory).join(format!("{name}.svg")), svg)
                .expect("write quality corpus SVG reference");
            fs::write(Path::new(directory).join(format!("{name}.png")), png)
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
    let x = u32::from(lead_column) * 12;
    assert!(wide_svg.contains(&format!(
        "<rect x=\"{x}\" y=\"96\" width=\"24\" height=\"24\""
    )));
    assert!(wide_svg.contains(&format!("<text x=\"{x}\" y=\"116\" fill=\"#000000\"")));
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
        assert!(
            first_rgba
                .chunks_exact(4)
                .all(|pixel| return pixel[3] == u8::MAX)
        );
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
