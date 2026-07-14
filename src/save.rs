//! Atomic multi-format publication for one semantic observation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{commit_directory, create_private_directory, sync_directory, write_secure},
    diff::inspect_point,
    event::ByteString,
    event_log::EventLog,
    ghostty_vt::Format,
    observation::{StatePoint, observe_session_with_options},
    replay::{StateSelector, format_state},
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
}

/// Atomic save result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveOutcome {
    /// Published directory.
    pub directory: PathBuf,
    /// Published manifest.
    pub manifest: SaveManifest,
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
    let observation = observe_session_with_options(&canonical, point.clone(), include_hidden)?;
    let inspection = inspect_point(&log, &point)?;
    if observation.state_hash != inspection.record.state_hash {
        return Err(save_error(
            "observation and deep replay resolved different states",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return save_error("save path has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return save_error("save path has no basename"))?;
    if !parent.is_dir() {
        return Err(save_error("save parent directory does not exist"));
    }
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<SaveManifest> {
        let mut files = Vec::new();
        for format in formats {
            let (path, media_type, bytes) = match format {
                SaveFormat::Txt => (
                    "screen.txt",
                    "text/plain; charset=utf-8",
                    observation.frame.viewport_text.as_bytes().to_vec(),
                ),
                SaveFormat::Json => (
                    "observation.json",
                    "application/json",
                    pretty_json(&observation, "observation.json")?,
                ),
                SaveFormat::DeepJson => (
                    "semantic-state.json",
                    "application/json",
                    pretty_json(&inspection, "semantic-state.json")?,
                ),
                SaveFormat::Vt => (
                    "screen.vt",
                    "application/vnd.termtrace.vt",
                    format_state(
                        &log,
                        StateSelector::StateId(inspection.record.state_id),
                        Format::Vt,
                    )?,
                ),
                SaveFormat::Html => (
                    "screen.html",
                    "text/html; charset=utf-8",
                    format_state(
                        &log,
                        StateSelector::StateId(inspection.record.state_id),
                        Format::Html,
                    )?,
                ),
            };
            let target = staging.join(path);
            write_secure(&target, &bytes)?;
            files.push(SavedFile {
                path: path.to_owned(),
                media_type: media_type.to_owned(),
                byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                blake3: blake3::hash(&bytes).to_hex().to_string(),
            });
        }
        let save_manifest = SaveManifest {
            format: "termtrace-save".to_owned(),
            version: 1,
            session_id: manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            state_point: point,
            state_id: inspection.record.state_id,
            state_hash: inspection.record.state_hash,
            source_final_event_hash: completion.final_event_hash,
            sensitivity: "terminal_screen_may_contain_secrets".to_owned(),
            files,
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

fn save_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
