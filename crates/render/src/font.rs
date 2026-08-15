//! Identity and loading for the bundled deterministic semantic font.

use std::sync::{Arc, LazyLock};

use resvg::usvg::fontdb::Database;

/// Bundled deterministic font family exposed in semantic SVG output.
pub const SEMANTIC_FONT_FAMILY: &str = "Termtrace Fira Code";
/// Font family recorded in the bundled font and used for local rasterization.
pub(super) const SEMANTIC_RASTER_FONT_FAMILY: &str = "Fira Code";
/// Repository-relative source of the bundled font.
pub const SEMANTIC_FONT_SOURCE: &str = "vendor/ghostty/pkg/freetype/res/FiraCode-Regular.ttf";
/// Exact bundled font bytes used by SVG and PNG output.
pub const SEMANTIC_FONT_BYTES: &[u8] =
    include_bytes!("../../../vendor/ghostty/pkg/freetype/res/FiraCode-Regular.ttf");

/// BLAKE3 identity of the exact bundled semantic font.
#[must_use]
pub fn semantic_font_hash() -> String {
    return blake3::hash(SEMANTIC_FONT_BYTES).to_hex().to_string();
}
pub(super) fn semantic_font_database() -> &'static Arc<Database> {
    static DATABASE: LazyLock<Arc<Database>> = LazyLock::new(|| {
        let mut database = Database::new();
        database.load_font_data(SEMANTIC_FONT_BYTES.to_vec());
        return Arc::new(database);
    });
    return &DATABASE;
}
