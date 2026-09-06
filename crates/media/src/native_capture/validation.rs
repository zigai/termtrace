use std::{
    fs::{self, DirEntry, File, OpenOptions, ReadDir},
    io::{self, BufRead as _, BufReader, Read as _},
    mem::size_of,
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use nix::libc::{O_NOFOLLOW, O_NONBLOCK};
use serde::de::DeserializeOwned;
use termtrace_model::terminal::replay::ReplayEndpoint;

use termtrace_engine::analysis::diagnostics::ValidatedVisualManifest;
use termtrace_evidence::{event_log::EventLogSummary, session::read_manifest};
use termtrace_model::artifact::visual::VideoMapEntry;

use termtrace_model::artifact::visual::{
    GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION, VisualManifest,
};

use super::{Error, Result};

/// Verify a persistent-worker screenshot and its exact canonical replay endpoint.
///
/// Live prefix captures remain verifiable after recording completes or grows. The
/// recorded prefix commitment, semantic endpoint, visual configuration and payload
/// hashes are checked independently; pixel presentation remains a sampling claim.
///
/// # Errors
/// Returns malformed-artifact, source-validation, replay, or cancellation failures.
pub fn validate_replay_screenshot(
    session: &Path,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<ReplayEndpoint> {
    use termtrace_engine::observation::{replay_endpoint_for_prefix, replay_endpoint_for_session};
    use termtrace_evidence::{artifact::verify_artifact_members, event_log::EventLogCursor};
    use termtrace_model::terminal::observation::StatePoint;

    check_review_cancelled(cancelled)?;
    let manifest: VisualManifest = bounded_json(&directory.join("manifest.json"))?;
    let endpoint: ReplayEndpoint = bounded_json(&directory.join("replay-endpoint.json"))?;
    if !manifest
        .members
        .iter()
        .any(|member| return member.path == "replay-endpoint.json")
    {
        return Err(invalid(
            "native screenshot has no committed replay endpoint",
        ));
    }
    verify_artifact_members(directory, &manifest.members)?;
    check_review_cancelled(cancelled)?;
    let canonical = read_manifest(session)?;
    let header = EventLogCursor::open(session.join(&canonical.event_log))?;
    validate_visual_manifest_binding(
        &manifest,
        canonical
            .ghostty_visual
            .as_ref()
            .map(|profile| return profile.effective_config_hash.as_str()),
        &header.header().colors.source,
    )?;
    let actual = if endpoint.source_complete {
        let point = endpoint
            .state_id
            .map_or(StatePoint::LatestPresented, |state_id| {
                return StatePoint::StateId { state_id };
            });
        replay_endpoint_for_session(session, &point, cancelled)?
    } else {
        replay_endpoint_for_prefix(session, &endpoint.boundary, cancelled)?
    };
    if endpoint != actual
        || manifest.capture_kind != "screenshot"
        || manifest.session_id != endpoint.session_id
        || manifest.source_event_hash.as_deref() != Some(endpoint.boundary.event_hash.as_str())
        || manifest.selected_state_id != endpoint.state_id
        || manifest.selected_state_hash.as_deref() != Some(endpoint.state_hash.as_str())
        || manifest.endpoint_event_seq != endpoint.event_seq
        || manifest.endpoint_event_byte_offset != endpoint.event_byte_offset
        || (manifest.geometry_report.cols, manifest.geometry_report.rows)
            != (endpoint.cols, endpoint.rows)
    {
        return Err(invalid(
            "native screenshot does not match its canonical replay endpoint",
        ));
    }
    return Ok(endpoint);
}

fn bounded_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = Vec::new();
    open_review_file(path)
        .and_then(|file| return file.take(1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid("native artifact JSON exceeds 1 MiB"));
    }
    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

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
    let canonical_log = EventLogSummary::read(session.join(&session_manifest.event_log))?;
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

/// Load only native correlations for one exact state with bounded retained data.
///
/// Canonical completion is authenticated by the shared observation verifier.
/// Candidate manifests and video-map lines have explicit allocation bounds, and
/// map scans check cancellation without retaining unrelated semantic states.
pub fn validated_visual_manifests_for_state(
    session: &Path,
    state_id: u64,
    cancelled: &AtomicBool,
) -> Result<Vec<ValidatedVisualManifest>> {
    use termtrace_engine::observation::{
        StatePoint, presented_replay_endpoint_for_session,
        semantic_state_with_hash_for_session_cancellable,
    };
    use termtrace_evidence::{event_log::EventLogCursor, session::read_completion};

    semantic_state_with_hash_for_session_cancellable(
        session,
        &StatePoint::LatestPresented,
        false,
        cancelled,
    )?;
    let session_manifest = read_manifest(session)?;
    let completion = read_completion(session)?;
    let Some(selected_endpoint) = presented_replay_endpoint_for_session(
        session,
        &StatePoint::StateId { state_id },
        cancelled,
    )?
    else {
        return Ok(Vec::new());
    };
    let canonical = EventLogCursor::open(session.join(&session_manifest.event_log))?;
    let expected_profile_hash = session_manifest
        .ghostty_visual
        .as_ref()
        .map(|profile| return profile.effective_config_hash.as_str());
    let directory = VisualDirectories::open(session).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let mut validated = Vec::new();
    let mut retained_bytes = 0_usize;
    for entry in directory {
        check_review_cancelled(cancelled)?;
        let entry = entry.map_err(|source| {
            return Error::Filesystem {
                path: session.to_path_buf(),
                source,
            };
        })?;
        if !entry
            .file_type()
            .map_err(|source| {
                return Error::Filesystem {
                    path: entry.path(),
                    source,
                };
            })?
            .is_dir()
        {
            continue;
        }
        let manifest_path = entry.path().join("manifest.json");
        let mut file = match open_review_file(&manifest_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(Error::Filesystem {
                    path: manifest_path,
                    source,
                });
            }
        };
        let mut bytes = Vec::new();
        file.by_ref()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| {
                return Error::Filesystem {
                    path: manifest_path.clone(),
                    source,
                };
            })?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("reviewer native manifest exceeds 1 MiB limit"));
        }
        let Ok(manifest) = serde_json::from_slice::<VisualManifest>(&bytes) else {
            continue;
        };
        let replay_screenshot = manifest
            .members
            .iter()
            .any(|member| return member.path == "replay-endpoint.json");
        if manifest.session_id != session_manifest.session_id
            || !replay_screenshot
                && manifest.source_event_hash.as_deref()
                    != Some(completion.final_event_hash.as_str())
            || validate_visual_manifest_binding(
                &manifest,
                expected_profile_hash,
                &canonical.header().colors.source,
            )
            .is_err()
        {
            continue;
        }
        let resolved_screenshot = if replay_screenshot {
            let endpoint = validate_replay_screenshot(session, &entry.path(), cancelled)?;
            endpoint.state_hash == selected_endpoint.state_hash
                && endpoint.event_seq == selected_endpoint.event_seq
                && endpoint.event_byte_offset == selected_endpoint.event_byte_offset
        } else {
            false
        };
        let map_path = entry.path().join("video-map.jsonl");
        let mut video_map = Vec::new();
        match open_review_file(&map_path) {
            Ok(file) => {
                let member = manifest
                    .members
                    .iter()
                    .find(|member| return member.path == "video-map.jsonl")
                    .ok_or_else(|| {
                        return invalid("native video map is absent from its manifest");
                    })?;
                let mut reader = BufReader::new(file);
                let mut hash = blake3::Hasher::new();
                let mut total_bytes = 0_u64;
                let mut line = Vec::new();
                loop {
                    check_review_cancelled(cancelled)?;
                    line.clear();
                    let count = reader
                        .by_ref()
                        .take(64 * 1024 + 1)
                        .read_until(b'\n', &mut line)
                        .map_err(|source| {
                            return Error::Filesystem {
                                path: map_path.clone(),
                                source,
                            };
                        })?;
                    if count == 0 {
                        break;
                    }
                    if count > 64 * 1024 {
                        return Err(invalid("native video-map line exceeds 64 KiB limit"));
                    }
                    hash.update(&line);
                    total_bytes =
                        total_bytes.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
                    let mapped: VideoMapEntry = serde_json::from_slice(&line).map_err(|error| {
                        return invalid(format!("invalid native video-map entry: {error}"));
                    })?;
                    if mapped.state_id == state_id {
                        if video_map.len() >= 4096 {
                            return Err(invalid(
                                "selected state exceeds 4096 native frame correlations",
                            ));
                        }
                        video_map.push(mapped);
                    }
                }
                if total_bytes != member.byte_length
                    || hash.finalize().to_hex().as_str() != member.blake3
                {
                    return Err(invalid(
                        "native video-map content does not match its manifest",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Filesystem {
                    path: map_path,
                    source,
                });
            }
        }
        if manifest.selected_state_id != Some(state_id)
            && !resolved_screenshot
            && video_map.is_empty()
        {
            continue;
        }
        retained_bytes = retained_bytes
            .saturating_add(bytes.len())
            .saturating_add(video_map.len().saturating_mul(size_of::<VideoMapEntry>()));
        if validated.len() >= 256 || retained_bytes > 8 * 1024 * 1024 {
            return Err(invalid(
                "selected native evidence exceeds reviewer retention budget",
            ));
        }
        let relative = manifest_path
            .strip_prefix(session)
            .unwrap_or(&manifest_path)
            .to_path_buf();
        let mut visual =
            ValidatedVisualManifest::new(relative.clone(), manifest, video_map, |_| return Ok(()))
                .map_err(Error::from)?;
        if resolved_screenshot {
            visual.bind_screenshot_state(state_id, &selected_endpoint)?;
        }
        validated.push((relative, visual));
    }
    check_review_cancelled(cancelled)?;
    semantic_state_with_hash_for_session_cancellable(
        session,
        &StatePoint::LatestPresented,
        false,
        cancelled,
    )?;
    if read_completion(session)?.final_event_hash != completion.final_event_hash {
        return Err(invalid(
            "canonical source changed during native reviewer query",
        ));
    }
    validated.sort_unstable_by(|(left, _), (right, _)| return left.cmp(right));
    return Ok(validated
        .into_iter()
        .map(|(_, visual)| return visual)
        .collect());
}

fn check_review_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(invalid("native reviewer query cancelled"));
    }
    return Ok(());
}

/// Retain only two directory iterators while visiting standalone and batch captures.
struct VisualDirectories {
    root: ReadDir,
    batch: Option<ReadDir>,
}

impl VisualDirectories {
    fn open(session: &Path) -> io::Result<Self> {
        return Ok(Self {
            root: fs::read_dir(session)?,
            batch: None,
        });
    }
}

impl Iterator for VisualDirectories {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(batch) = &mut self.batch {
            if let Some(entry) = batch.next() {
                return Some(entry);
            }
            self.batch = None;
        }
        let entry = match self.root.next()? {
            Ok(entry) => entry,
            Err(error) => return Some(Err(error)),
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && entry.path().join("batch-manifest.json").is_file() => {
                match fs::read_dir(entry.path()) {
                    Ok(batch) => self.batch = Some(batch),
                    Err(error) => return Some(Err(error)),
                }
            }
            Ok(_) => {}
            Err(error) => return Some(Err(error)),
        }
        return Some(Ok(entry));
    }
}

fn open_review_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "review evidence member must be a regular file",
        ));
    }
    return Ok(file);
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
