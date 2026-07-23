//! Atomic multi-format publication for one semantic observation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{commit_directory, create_private_directory, sync_directory, write_secure},
    event::ByteString,
    event_log::EventLog,
    ghostty_vt::Format,
    observation::{
        ObservationResolution, StatePoint, completed_observation_from_inspection, state_selector,
    },
    replay::{inspect_many, inspect_with_formats},
    semantic_image::{
        SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_BYTES, SEMANTIC_FONT_FAMILY,
        SEMANTIC_FONT_SIZE, SEMANTIC_FONT_SOURCE, SEMANTIC_PNG_RENDERER, SEMANTIC_SVG_RENDERER,
        render_png, render_svg, semantic_font_hash,
    },
    session::{read_manifest, validate_completed_log},
};

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

/// Manifest for an atomically published multi-state screenshot batch.
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

/// Resolve one state once and atomically publish selected representations.
///
/// # Errors
///
/// Returns an error for invalid/incomplete evidence, unsupported hidden state,
/// existing destination, filesystem failure, or inconsistent state resolution.
pub fn save(
    session: &Path,
    destination: &Path,
    point: StatePoint,
    formats: &[SaveFormat],
    include_hidden: bool,
) -> Result<SaveOutcome> {
    if formats.is_empty() {
        return Err(save_error("at least one save format is required"));
    }
    if formats.iter().enumerate().any(|(index, format)| {
        return formats
            .iter()
            .skip(index + 1)
            .any(|later| return later == format);
    }) {
        return Err(save_error("save formats must not be repeated"));
    }
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let manifest = read_manifest(&canonical)?;
    let engine_formats = formats
        .iter()
        .filter_map(|format| match format {
            SaveFormat::Vt => return Some(Format::Vt),
            SaveFormat::Html => return Some(Format::Html),
            _ => return None,
        })
        .collect::<Vec<_>>();
    let selector = state_selector(&log, &point)?;
    let (inspection, rendered) = inspect_with_formats(&log, selector, &engine_formats)?;
    if !inspection.record.presented && !include_hidden {
        return Err(save_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let observation = completed_observation_from_inspection(
        &canonical,
        manifest.session_id,
        point.clone(),
        &completion,
        &inspection,
        ObservationResolution::CanonicalReplay,
    )?;
    let parent = destination
        .parent()
        .ok_or_else(|| return save_error("save path has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return save_error("save path has no basename"))?;
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<SaveManifest> {
        let mut files = Vec::new();
        for format in formats {
            let (path, media_type, bytes, truth_layer, renderer) = match format {
                SaveFormat::Txt => (
                    "screen.txt",
                    "text/plain; charset=utf-8",
                    observation.frame.viewport_text.as_bytes().to_vec(),
                    None,
                    None,
                ),
                SaveFormat::Json => (
                    "observation.json",
                    "application/json",
                    pretty_json(&observation, "observation.json")?,
                    None,
                    None,
                ),
                SaveFormat::DeepJson => (
                    "semantic-state.json",
                    "application/json",
                    pretty_json(&inspection, "semantic-state.json")?,
                    None,
                    None,
                ),
                SaveFormat::Vt => (
                    "screen.vt",
                    "application/vnd.termtrace.vt",
                    rendered_format(&rendered, Format::Vt)?,
                    Some("semantic"),
                    Some("pinned-libghostty-vt-formatter"),
                ),
                SaveFormat::Html => (
                    "screen.html",
                    "text/html; charset=utf-8",
                    rendered_format(&rendered, Format::Html)?,
                    Some("semantic"),
                    Some("pinned-libghostty-vt-formatter"),
                ),
                SaveFormat::Svg => (
                    "screen.svg",
                    "image/svg+xml",
                    render_svg(&inspection.state),
                    Some("semantic_rendering"),
                    Some(SEMANTIC_SVG_RENDERER),
                ),
                SaveFormat::Png => (
                    "screen.png",
                    "image/png",
                    render_png(&inspection.state)?,
                    Some("semantic_rendering"),
                    Some(SEMANTIC_PNG_RENDERER),
                ),
            };
            let target = staging.join(path);
            write_secure(&target, &bytes)?;
            files.push(SavedFile {
                path: path.to_owned(),
                media_type: media_type.to_owned(),
                byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                blake3: blake3::hash(&bytes).to_hex().to_string(),
                truth_layer: truth_layer.map(str::to_owned),
                renderer: renderer.map(str::to_owned),
            });
        }
        let save_manifest = SaveManifest {
            format: "termtrace-save".to_owned(),
            version: 2,
            session_id: manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            state_point: point,
            state_id: inspection.record.state_id,
            state_hash: inspection.record.state_hash,
            source_final_event_hash: completion.final_event_hash,
            sensitivity: "terminal_screen_may_contain_secrets".to_owned(),
            files,
            semantic_image_resources: if formats
                .iter()
                .any(|format| matches!(format, SaveFormat::Svg | SaveFormat::Png))
            {
                semantic_image_resources()
            } else {
                Vec::new()
            },
        };
        write_secure(
            &staging.join("save-manifest.json"),
            &pretty_json(&save_manifest, "save-manifest.json")?,
        )?;
        sync_directory(&staging)?;
        return Ok(save_manifest);
    })();
    let save_manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error);
    }
    return Ok(SaveOutcome {
        directory: destination.to_path_buf(),
        manifest: save_manifest,
    });
}

/// Atomically publish several exact semantic states under one batch directory.
///
/// # Errors
///
/// Returns the same errors as [`save`] and rejects empty or non-image batches.
pub fn save_screenshot_batch(
    session: &Path,
    destination: &Path,
    points: &[StatePoint],
    format: SaveFormat,
    include_hidden: bool,
) -> Result<ScreenshotBatchManifest> {
    if points.is_empty() || !matches!(format, SaveFormat::Svg | SaveFormat::Png) {
        return Err(save_error(
            "screenshot batches require at least one point and an SVG or PNG format",
        ));
    }
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let source_manifest = read_manifest(&canonical)?;
    let selectors = points
        .iter()
        .map(|point| return state_selector(&log, point))
        .collect::<Result<Vec<_>>>()?;
    let inspections = inspect_many(&log, &selectors)?;
    if !include_hidden
        && inspections
            .iter()
            .any(|inspection| return !inspection.record.presented)
    {
        return Err(save_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return save_error("batch path has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return save_error("batch path has no basename"))?;
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<ScreenshotBatchManifest> {
        let filename = match format {
            SaveFormat::Svg => "screen.svg",
            SaveFormat::Png => "screen.png",
            _ => return Err(save_error("batch format must be SVG or PNG")),
        };
        let mut entries = Vec::with_capacity(points.len());
        for (index, (point, inspection)) in points.iter().zip(inspections).enumerate() {
            let directory = format!("state-{index:04}");
            let entry_directory = staging.join(&directory);
            create_private_directory(&entry_directory)?;
            let (bytes, media_type, renderer) = match format {
                SaveFormat::Svg => (
                    render_svg(&inspection.state),
                    "image/svg+xml",
                    SEMANTIC_SVG_RENDERER,
                ),
                SaveFormat::Png => (
                    render_png(&inspection.state)?,
                    "image/png",
                    SEMANTIC_PNG_RENDERER,
                ),
                _ => return Err(save_error("batch format must be SVG or PNG")),
            };
            write_secure(&entry_directory.join(filename), &bytes)?;
            let save_manifest = SaveManifest {
                format: "termtrace-save".to_owned(),
                version: 2,
                session_id: source_manifest.session_id,
                canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
                state_point: point.clone(),
                state_id: inspection.record.state_id,
                state_hash: inspection.record.state_hash,
                source_final_event_hash: completion.final_event_hash.clone(),
                sensitivity: "terminal_screen_may_contain_secrets".to_owned(),
                files: vec![SavedFile {
                    path: filename.to_owned(),
                    media_type: media_type.to_owned(),
                    byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    blake3: blake3::hash(&bytes).to_hex().to_string(),
                    truth_layer: Some("semantic_rendering".to_owned()),
                    renderer: Some(renderer.to_owned()),
                }],
                semantic_image_resources: semantic_image_resources(),
            };
            write_secure(
                &entry_directory.join("save-manifest.json"),
                &pretty_json(&save_manifest, "save-manifest.json")?,
            )?;
            sync_directory(&entry_directory)?;
            entries.push(ScreenshotBatchEntry {
                image: format!("{directory}/{filename}"),
                directory,
                manifest: save_manifest,
            });
        }
        let manifest = ScreenshotBatchManifest {
            format: "termtrace-screenshot-batch".to_owned(),
            version: 1,
            image_format: format,
            entries,
        };
        write_secure(
            &staging.join("batch-manifest.json"),
            &pretty_json(&manifest, "batch-manifest.json")?,
        )?;
        sync_directory(&staging)?;
        return Ok(manifest);
    })();
    let manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error);
    }
    return Ok(manifest);
}

fn semantic_image_resources() -> Vec<SemanticImageResource> {
    return vec![SemanticImageResource {
        font_family: SEMANTIC_FONT_FAMILY.to_owned(),
        font_source: SEMANTIC_FONT_SOURCE.to_owned(),
        font_byte_count: u64::try_from(SEMANTIC_FONT_BYTES.len()).unwrap_or(u64::MAX),
        font_blake3: semantic_font_hash(),
        fallback_policy: "bundled_font_only_missing_glyph_box".to_owned(),
        font_size: SEMANTIC_FONT_SIZE,
        cell_width: SEMANTIC_CELL_WIDTH,
        cell_height: SEMANTIC_CELL_HEIGHT,
        padding: 0,
        background_policy: "opaque_terminal_background".to_owned(),
    }];
}

fn pretty_json(value: &impl Serialize, path: &str) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from(path),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    bytes.push(b'\n');
    return Ok(bytes);
}

fn rendered_format(rendered: &[(Format, Vec<u8>)], format: Format) -> Result<Vec<u8>> {
    return rendered
        .iter()
        .find(|(candidate, _)| return *candidate == format)
        .map(|(_, bytes)| return bytes.clone())
        .ok_or_else(|| return save_error("requested semantic format was not captured"));
}

fn save_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
