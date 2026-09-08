//! Deterministic semantic terminal image rendering.

#![deny(unsafe_code)]

pub mod diff;
mod error;
pub mod font;
pub mod raster;
pub mod semantic_svg;

#[cfg(test)]
mod quality_corpus;

pub use diff::{render_diff_png, render_diff_svg, SEMANTIC_DIFF_SVG_RENDERER};
pub use error::{Error, Result};
pub use font::{semantic_font_set, SemanticFontSet};
pub use raster::{render_png, render_png_with_profile, render_video_rgba, SEMANTIC_PNG_RENDERER};
pub use semantic_svg::{
    render_svg, SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_SIZE,
    SEMANTIC_SVG_RENDERER, SEMANTIC_SYMBOLS_BASELINE, SEMANTIC_SYMBOLS_FONT_SIZE,
    SEMANTIC_TEXT_BASELINE,
};
