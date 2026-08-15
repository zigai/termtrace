use std::{
    fs::File,
    io::{self, BufRead as _, BufReader},
    path::Path,
};

use termtrace_engine::analysis::diagnostics::ValidatedVisualManifest;
use termtrace_evidence::{event_log::EventLog, session::read_manifest};
use termtrace_model::artifact::visual::VideoMapEntry;

use termtrace_model::artifact::visual::{
    GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION, VisualManifest,
};

use super::{Error, Result};

/// Validate the version and effective-configuration member binding of a visual manifest.
///
/// This checks manifest structure only; use [`validated_visual_manifests`] to bind manifests to
/// a session and verify their artifact members.
pub fn validate_visual_manifest(manifest: &VisualManifest) -> Result<()> {
    if manifest.version != VISUAL_MANIFEST_VERSION {
        return Err(invalid(format!(
            "unsupported native visual manifest version {}; expected {VISUAL_MANIFEST_VERSION}",
            manifest.version
        )));
    }
    if manifest.effective_config_hash.len() != 64
        || !manifest
            .effective_config_hash
            .bytes()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "native visual effective configuration hash must be 64 lowercase hexadecimal characters",
        ));
    }
    let mut members = manifest
        .members
        .iter()
        .filter(|member| return member.path == GHOSTTY_EFFECTIVE_CONFIG_NAME);
    let member = members.next().ok_or_else(|| {
        return invalid(format!(
            "native visual manifest is missing `{GHOSTTY_EFFECTIVE_CONFIG_NAME}`"
        ));
    })?;
    if members.next().is_some() {
        return Err(invalid(format!(
            "native visual manifest contains duplicate `{GHOSTTY_EFFECTIVE_CONFIG_NAME}` members"
        )));
    }
    if member.blake3 != manifest.effective_config_hash {
        return Err(invalid(format!(
            "native visual effective configuration hash does not match the `{GHOSTTY_EFFECTIVE_CONFIG_NAME}` member"
        )));
    }
    return Ok(());
}

pub(crate) fn validate_visual_manifest_binding(
    manifest: &VisualManifest,
    recorded_profile_hash: Option<&str>,
    color_source: &str,
) -> Result<()> {
    validate_visual_manifest(manifest)?;
    if let Some(expected) = recorded_profile_hash
        .or_else(|| return color_source.strip_prefix("ghostty_show_config_blake3:"))
        && manifest.effective_config_hash != expected
    {
        return Err(invalid(format!(
            "native visual effective configuration hash does not match canonical session evidence (recorded {expected}, visual {})",
            manifest.effective_config_hash
        )));
    }
    return Ok(());
}

/// Load native manifests whose session and effective configuration bindings are valid.
///
/// Non-visual directories and malformed untrusted candidate manifests are ignored.
///
/// # Errors
///
/// Returns an error when the session or a selected visual artifact cannot be read safely.
pub fn validated_visual_manifests(
    session: impl AsRef<Path>,
) -> Result<Vec<ValidatedVisualManifest>> {
    let session = session.as_ref();
    let session_manifest = read_manifest(session)?;
    let canonical_log = EventLog::read(session.join(&session_manifest.event_log))?;
    let expected_profile_hash = session_manifest
        .ghostty_visual
        .as_ref()
        .map(|profile| return profile.effective_config_hash.as_str());
    let mut entries: Vec<_> = fs_err::read_dir(session)
        .map_err(|source| {
            return Error::Filesystem {
                path: session.to_path_buf(),
                source,
            };
        })?
        .collect::<io::Result<Vec<_>>>()
        .map_err(|source| {
            return Error::Filesystem {
                path: session.to_path_buf(),
                source,
            };
        })?;
    entries.sort_unstable_by_key(|entry| return entry.file_name());
    let mut validated: Vec<ValidatedVisualManifest> = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(|source| {
            return Error::Filesystem {
                path: entry.path(),
                source,
            };
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
            return Error::Filesystem {
                path: manifest_path.clone(),
                source,
            };
        })?;
        let Ok(manifest) = serde_json::from_slice::<VisualManifest>(&manifest_bytes) else {
            continue;
        };
        if manifest.session_id != session_manifest.session_id
            || validate_visual_manifest_binding(
                &manifest,
                expected_profile_hash,
                &canonical_log.header.colors.source,
            )
            .is_err()
        {
            continue;
        }
        let map_path = entry.path().join("video-map.jsonl");
        let mut video_map: Vec<VideoMapEntry> = Vec::new();
        if map_path.is_file() {
            let reader = BufReader::new(File::open(&map_path).map_err(|source| {
                return Error::Filesystem {
                    path: map_path.clone(),
                    source,
                };
            })?);
            for line in reader.lines() {
                let line = line.map_err(|source| {
                    return Error::Filesystem {
                        path: map_path.clone(),
                        source,
                    };
                })?;
                if let Ok(entry) = serde_json::from_str::<VideoMapEntry>(&line) {
                    video_map.push(entry);
                }
            }
        }
        let relative_manifest = manifest_path
            .strip_prefix(session)
            .unwrap_or(&manifest_path)
            .to_path_buf();
        let visual =
            ValidatedVisualManifest::new(relative_manifest, manifest, video_map, |_| return Ok(()))
                .map_err(Error::from)?;
        validated.push(visual);
    }
    return Ok(validated);
}

fn invalid(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use termtrace_model::{
        artifact::metadata::{ArtifactMember, ArtifactSensitivity},
        artifact::visual::VisualManifest,
        terminal::replay::StateSelection,
        value::geometry::TerminalGeometryReport,
    };
    use uuid::Uuid;

    use super::*;

    #[test]
    fn effective_configuration_commitment_is_strict() {
        let mut manifest = valid_manifest();
        validate_visual_manifest(&manifest).expect("valid native visual manifest");

        let mut obsolete = serde_json::to_value(&manifest).expect("visual manifest JSON");
        obsolete
            .as_object_mut()
            .expect("visual manifest object")
            .insert(
                "ghostty_config_hash".to_owned(),
                serde_json::json!("a".repeat(64)),
            );
        assert!(serde_json::from_value::<VisualManifest>(obsolete).is_err());

        manifest.version = VISUAL_MANIFEST_VERSION - 1;
        assert!(validate_visual_manifest(&manifest).is_err());
        manifest.version = VISUAL_MANIFEST_VERSION;

        manifest.effective_config_hash = "A".repeat(64);
        assert!(validate_visual_manifest(&manifest).is_err());
        manifest.effective_config_hash = "a".repeat(64);

        manifest.members[0].blake3 = "b".repeat(64);
        assert!(validate_visual_manifest(&manifest).is_err());
    }

    #[test]
    fn session_profile_hash_is_authoritative_over_the_color_fallback() {
        let manifest = valid_manifest();
        validate_visual_manifest_binding(
            &manifest,
            Some(&"a".repeat(64)),
            &format!("ghostty_show_config_blake3:{}", "b".repeat(64)),
        )
        .expect("recorded profile hash is the sole authority when present");

        assert!(
            validate_visual_manifest_binding(
                &manifest,
                None,
                &format!("ghostty_show_config_blake3:{}", "b".repeat(64)),
            )
            .is_err()
        );
    }

    fn valid_manifest() -> VisualManifest {
        let effective_config_hash = "a".repeat(64);
        return VisualManifest {
            version: VISUAL_MANIFEST_VERSION,
            session_id: Uuid::nil(),
            source_event_hash: None,
            source_range: None,
            state_selection: StateSelection::All,
            geometry_report: TerminalGeometryReport {
                cols: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 16,
                text_width: 640,
                text_height: 384,
            },
            fps: 24,
            width: 640,
            height: 384,
            capture_started_unix_ns: 0,
            replay_origin_unix_ns: 0,
            effective_config_hash: effective_config_hash.clone(),
            ghostty_version: "test".to_owned(),
            ffmpeg_version: None,
            ghostty_overrides: Vec::new(),
            video_probe: None,
            screenshot_probe: serde_json::json!({}),
            members: vec![ArtifactMember {
                path: GHOSTTY_EFFECTIVE_CONFIG_NAME.to_owned(),
                byte_length: 0,
                blake3: effective_config_hash,
                media_type: "text/plain".to_owned(),
                schema_type: None,
                sensitivity: ArtifactSensitivity::OperationalMetadata,
                relationship: "capture_environment".to_owned(),
            }],
            capture_kind: "screenshot".to_owned(),
            selected_state_id: None,
            selected_state_hash: None,
            endpoint_event_seq: None,
            endpoint_event_byte_offset: None,
            presentation_evidence: "test".to_owned(),
            frame_samples: Vec::new(),
            guarantee_level: "native_capture_environment".to_owned(),
        };
    }
}
