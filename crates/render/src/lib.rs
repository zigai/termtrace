//! Deterministic semantic terminal image rendering.

#![deny(unsafe_code)]

pub mod diff;
mod error;
pub mod font;
pub mod raster;
pub mod semantic_svg;

pub use diff::{SEMANTIC_DIFF_SVG_RENDERER, render_diff_png, render_diff_svg};
pub use error::{Error, Result};
pub use font::{
    SEMANTIC_FONT_BYTES, SEMANTIC_FONT_FAMILY, SEMANTIC_FONT_SOURCE, semantic_font_hash,
};
pub use raster::{SEMANTIC_PNG_RENDERER, render_png, render_video_rgba};
pub use semantic_svg::{
    SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_SIZE, SEMANTIC_SVG_RENDERER,
    render_svg,
};
