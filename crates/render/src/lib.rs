//! Deterministic semantic terminal image rendering.

#![deny(unsafe_code)]

pub mod diff;
mod error;
pub mod font;
pub mod raster;
pub mod semantic_svg;

#[cfg(test)]
mod quality_corpus;

pub use diff::{SEMANTIC_DIFF_SVG_RENDERER, render_diff_png, render_diff_svg};
pub use error::{Error, Result};
pub use font::{SemanticFontSet, semantic_font_set};
pub use raster::{SEMANTIC_PNG_RENDERER, render_png, render_png_with_profile, render_video_rgba};
pub use semantic_svg::{
    SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_SIZE, SEMANTIC_SVG_RENDERER,
    SEMANTIC_SYMBOLS_BASELINE, SEMANTIC_SYMBOLS_FONT_SIZE, SEMANTIC_TEXT_BASELINE, render_svg,
};
