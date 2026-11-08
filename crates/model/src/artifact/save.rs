//! Atomic multi-format publication for one semantic observation.

use crate::{terminal::observation::StatePoint, value::ByteString};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Semantic file recipe supported by `save`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SaveFormat {
    /// Plain viewport text.
    Txt,
    /// Compact truth-labeled observation JSON.
    Json,
    /// Complete reconstructed semantic-state JSON.
    DeepJson,
    /// Pinned Ghostty VT reconstruction.
    Vt,
    /// Pinned Ghostty HTML reconstruction.
    Html,
    /// Deterministic semantic-state SVG.
    Svg,
    /// Deterministic semantic-state PNG rasterized from the SVG scene.
    Png,
}

/// Stable role of one bundled semantic font face.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticFontRole {
    /// Plain JetBrains Mono text.
    Regular,
    /// Bold JetBrains Mono text.
    Bold,
    /// Italic JetBrains Mono text.
    Italic,
    /// Bold italic JetBrains Mono text.
    BoldItalic,
    /// Symbols Nerd Font fallback.
    SymbolsFallback,
}

impl SemanticFontRole {
    /// Stable serialized role identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regular => return "regular",
            Self::Bold => return "bold",
            Self::Italic => return "italic",
            Self::BoldItalic => return "bold-italic",
            Self::SymbolsFallback => return "symbols-fallback",
        }
    }
}

/// Deterministic semantic rasterization quality profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticRasterProfile {
    /// Rasterize directly at the target dimensions.
    Direct,
    /// Rasterize at twice the target dimensions and downsample once.
    #[serde(rename = "supersampled-2x")]
    Supersampled2x,
}

/// Exact identity of one bundled semantic font face.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticFontResource {
    /// Stable face role.
    pub role: SemanticFontRole,
    /// Termtrace-scoped family used by semantic renderers.
    pub family: String,
    /// Repository-relative source asset.
    pub repository_source: String,
    /// Decompressed TTF byte count.
    pub decompressed_size: u64,
    /// BLAKE3 of the decompressed TTF bytes.
    pub decompressed_blake3: String,
}

/// Exact font and layout resources used by semantic image output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticImageResource {
    /// Ordered bundled font resources in serialized role order.
    pub fonts: Vec<SemanticFontResource>,
    /// Aggregate BLAKE3 identity of all decompressed bundled faces.
    pub font_set_blake3: String,
    /// Deterministic missing-glyph policy.
    pub fallback_policy: String,
    /// Deterministic rasterization quality profile.
    pub raster_profile: SemanticRasterProfile,
    /// Font size in semantic SVG units.
    pub font_size: u16,
    /// Text baseline in semantic SVG units.
    pub text_baseline: u16,
    /// Symbols fallback font size in semantic SVG units.
    pub symbols_font_size: u16,
    /// Symbols fallback baseline in semantic SVG units.
    pub symbols_baseline: u16,
    /// Fixed semantic cell width.
    pub cell_width: u16,
    /// Fixed semantic cell height.
    pub cell_height: u16,
    /// Fixed outer padding.
    pub padding: u16,
    /// Background policy.
    pub background_policy: String,
}

/// One manifested saved file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedFile {
    /// Relative filename.
    pub path: String,
    /// Media type.
    pub media_type: String,
    /// Exact byte count.
    pub byte_count: u64,
    /// BLAKE3 of exact file bytes.
    pub blake3: String,
    /// Rendering truth layer, when this member is a rendering.
    pub truth_layer: Option<String>,
    /// Stable renderer identity, when applicable.
    pub renderer: Option<String>,
}

/// Manifest binding every saved representation to one source state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveManifest {
    /// Stable manifest format.
    pub format: String,
    /// Manifest version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Exact canonical session path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub canonical_session: ByteString,
    /// Requested semantic point.
    pub state_point: StatePoint,
    /// Resolved state identifier.
    pub state_id: u64,
    /// Resolved complete semantic state hash.
    pub state_hash: String,
    /// Final canonical event hash.
    pub source_final_event_hash: String,
    /// Explicit sensitivity classification.
    pub sensitivity: String,
    /// Manifested representations.
    pub files: Vec<SavedFile>,
    /// Exact deterministic resources when semantic images are present.
    pub semantic_image_resources: Vec<SemanticImageResource>,
}

/// Atomic save result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct SaveOutcome {
    /// Published directory.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub directory: PathBuf,
    /// Published manifest.
    pub manifest: SaveManifest,
}

/// One entry in an atomic multi-state semantic screenshot batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotBatchEntry {
    /// Batch-relative artifact directory.
    pub directory: String,
    /// Batch-relative image path.
    pub image: String,
    /// Complete per-state save manifest.
    pub manifest: SaveManifest,
}

/// Manifest for an atomically published multi-state semantic screenshot batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotBatchManifest {
    /// Stable batch format.
    pub format: String,
    /// Batch format version.
    pub version: u32,
    /// Semantic image format shared by every entry.
    pub image_format: SaveFormat,
    /// Ordered exact-state entries.
    pub entries: Vec<ScreenshotBatchEntry>,
}

#[cfg(test)]
mod tests {
    use super::SemanticRasterProfile;

    #[test]
    fn semantic_raster_profiles_have_exact_persisted_names() {
        assert_eq!(
            serde_json::to_string(&SemanticRasterProfile::Direct)
                .expect("serialize direct raster profile"),
            "\"direct\""
        );
        assert_eq!(
            serde_json::to_string(&SemanticRasterProfile::Supersampled2x)
                .expect("serialize supersampled raster profile"),
            "\"supersampled-2x\""
        );
    }
}
