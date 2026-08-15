//! Atomic multi-format publication for one semantic observation.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{terminal::observation::StatePoint, value::ByteString};

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

/// Exact font and layout resources used by semantic image output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticImageResource {
    /// Embedded font family.
    pub font_family: String,
    /// Repository-relative font source.
    pub font_source: String,
    /// Exact font byte count.
    pub font_byte_count: u64,
    /// BLAKE3 of exact font bytes.
    pub font_blake3: String,
    /// Deterministic missing-glyph policy.
    pub fallback_policy: String,
    /// Font size in semantic SVG units.
    pub font_size: u16,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveOutcome {
    /// Published directory.
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
