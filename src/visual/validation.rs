//! Native visual manifest validation.

use crate::{Error, Result};

use super::{GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION, VisualManifest};

/// Validate the native visual format and its effective Ghostty configuration commitment.
///
/// # Errors
///
/// Returns an error when the manifest version is unsupported, the effective
/// configuration hash is malformed, or the effective-configuration member is
/// missing, duplicated, or committed under a different hash.
pub fn validate_visual_manifest(manifest: &VisualManifest) -> Result<()> {
    if manifest.version != VISUAL_MANIFEST_VERSION {
        return Err(invalid_manifest(format!(
            "unsupported native visual manifest version {}; expected {VISUAL_MANIFEST_VERSION}",
            manifest.version
        )));
    }
    if !is_lowercase_blake3(&manifest.effective_config_hash) {
        return Err(invalid_manifest(
            "native visual effective configuration hash must be 64 lowercase hexadecimal characters",
        ));
    }
    let mut effective_members = manifest
        .members
        .iter()
        .filter(|member| return member.path == GHOSTTY_EFFECTIVE_CONFIG_NAME);
    let Some(effective_member) = effective_members.next() else {
        return Err(invalid_manifest(format!(
            "native visual manifest is missing `{GHOSTTY_EFFECTIVE_CONFIG_NAME}`"
        )));
    };
    if effective_members.next().is_some() {
        return Err(invalid_manifest(format!(
            "native visual manifest contains duplicate `{GHOSTTY_EFFECTIVE_CONFIG_NAME}` members"
        )));
    }
    if effective_member.blake3 != manifest.effective_config_hash {
        return Err(invalid_manifest(format!(
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
    let expected = recorded_profile_hash.or_else(|| {
        return color_source.strip_prefix("ghostty_show_config_blake3:");
    });
    if let Some(expected) = expected
        && manifest.effective_config_hash != expected
    {
        return Err(invalid_manifest(format!(
            "native visual effective configuration hash does not match canonical session evidence (recorded {expected}, visual {})",
            manifest.effective_config_hash
        )));
    }
    return Ok(());
}

fn is_lowercase_blake3(value: &str) -> bool {
    return value.len() == 64
        && value
            .bytes()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
}

fn invalid_manifest(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::{
        artifact::{ArtifactMember, ArtifactSensitivity},
        geometry::TerminalGeometryReport,
        replay::StateSelection,
    };

    #[test]
    fn effective_configuration_commitment_is_strict() {
        let mut manifest = valid_manifest();
        validate_visual_manifest(&manifest).expect("valid native visual manifest");

        let mut legacy = serde_json::to_value(&manifest).expect("visual manifest JSON");
        legacy
            .as_object_mut()
            .expect("visual manifest object")
            .insert(
                "ghostty_config_hash".to_owned(),
                serde_json::json!("a".repeat(64)),
            );
        assert!(serde_json::from_value::<VisualManifest>(legacy).is_err());

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
