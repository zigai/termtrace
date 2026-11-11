use super::{
    json_error, read_artifact_roots,
    secure_fs::{nix_filesystem, open_file_at, open_parent_directory, safe_file_name},
    ARTIFACT_MANIFEST,
};
use crate::event_log::EventLogCursor;
use crate::storage::{Error, Result};
use nix::{
    fcntl::OFlag,
    sys::stat::{fstat, Mode, SFlag},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Component, Path, PathBuf},
    thread,
};
use termtrace_model::artifact::metadata::{
    ArtifactMember, ArtifactRootRecord, ArtifactSensitivity, ArtifactSourceRange,
};
use termtrace_model::error::ThreadPanicError;
use termtrace_model::execution::{
    event::Event,
    event_log::CanonicalBoundary,
    layout::{self, SessionLayout},
};
use termtrace_model::terminal::replay::STATE_INDEX;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ArtifactMemberEnvelope {
    pub(crate) members: Vec<ArtifactMember>,
    pub(crate) source_event_hash: Option<String>,
    pub(crate) source_range: Option<ArtifactSourceRange>,
}

/// Inventory all regular payload files below an artifact directory.
///
/// The manifest itself is excluded from the returned members.
///
/// # Errors
///
/// Returns an error for unsafe entries, unreadable files, or paths that cannot be normalized.
pub fn collect_artifact_members(directory: &Path) -> Result<Vec<ArtifactMember>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_member_paths(directory, directory, &mut paths)?;
    paths.sort();
    let mut payloads: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());

    for relative in paths {
        let normalized = normalized_relative_path(&relative)?;
        if normalized != ARTIFACT_MANIFEST {
            payloads.push((directory.join(relative), normalized));
        }
    }

    let worker_count = available_workers(payloads.len());
    let mut indexed = thread::scope(|scope| -> Result<Vec<(usize, ArtifactMember)>> {
        let mut handles = Vec::with_capacity(worker_count);

        for worker in 0..worker_count {
            let payloads = &payloads;
            handles.push(
                scope.spawn(move || -> Result<Vec<(usize, ArtifactMember)>> {
                    let mut members: Vec<(usize, ArtifactMember)> = Vec::new();

                    for index in (worker..payloads.len()).step_by(worker_count) {
                        let (path, normalized) = payloads.get(index).ok_or_else(|| {
                            return Error::invalid("artifact hashing index is outside payloads");
                        })?;
                        let (byte_length, hash) = hash_regular_file(path)?;
                        members.push((
                            index,
                            ArtifactMember {
                                media_type: member_media_type(normalized).to_owned(),
                                schema_type: member_schema_type(normalized).map(str::to_owned),
                                sensitivity: ArtifactSensitivity::SensitiveTerminalData,
                                relationship: member_relationship(normalized).to_owned(),
                                path: normalized.clone(),
                                byte_length,
                                blake3: hash,
                            },
                        ));
                    }

                    return Ok(members);
                }),
            );
        }

        let mut indexed: Vec<(usize, ArtifactMember)> = Vec::with_capacity(payloads.len());
        for handle in handles {
            indexed.append(&mut handle.join().map_err(|payload| {
                return Error::invalid_with_source(
                    "artifact hashing worker panicked",
                    ThreadPanicError::from_payload(payload),
                );
            })??);
        }

        return Ok(indexed);
    })?;
    indexed.sort_by_key(|(index, _)| return *index);

    return Ok(indexed
        .into_iter()
        .map(|(_, member)| return member)
        .collect());
}

fn available_workers(item_count: usize) -> usize {
    return thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4)
        .min(item_count.max(1));
}

/// Verify an artifact directory against its expected whole-file member inventory.
///
/// # Errors
///
/// Returns an error when paths repeat or any payload metadata differs.
pub fn verify_artifact_members(directory: &Path, expected: &[ArtifactMember]) -> Result<()> {
    let actual = collect_artifact_members(directory)?;
    let expected_paths: BTreeSet<&str> = expected
        .iter()
        .map(|member| return member.path.as_str())
        .collect();

    if expected_paths.len() != expected.len() {
        return Err(Error::invalid(
            "artifact manifest contains duplicate member paths",
        ));
    }

    if actual != expected {
        return Err(Error::invalid(
            "artifact payload members do not match their whole-file manifest",
        ));
    }
    return Ok(());
}

/// Verify every derived artifact cataloged by a completed session.
///
/// Returns the number of verified artifact roots.
///
/// # Errors
///
/// Returns an error when a catalog root, manifest binding, or payload member is invalid.
pub fn verify_session_artifacts(
    session: &Path,
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
) -> Result<u64> {
    let records = read_artifact_roots(&SessionLayout::new(session).artifact_root_log())?;

    for record in &records {
        verify_artifact_root(session, record, session_manifest_hash, source_event_hash)?;
    }

    return Ok(u64::try_from(records.len()).unwrap_or(u64::MAX));
}

/// Open a derived artifact directory only when the session's catalog vouches for it.
///
/// This is the single trust rule for derived artifact directories: a catalog root must bind the
/// directory's exact manifest bytes to the given canonical source, and every payload member must
/// match that manifest. A directory without a catalog root is not trusted and yields `None`.
///
/// # Errors
///
/// Returns an error when a cataloged root, its manifest binding, or a payload member is invalid.
pub fn open_verified_artifact(
    session: &Path,
    artifact_directory: &str,
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
) -> Result<Option<PathBuf>> {
    let catalog = SessionLayout::new(session).artifact_root_log();
    if !catalog.try_exists().map_err(|source| {
        return Error::Filesystem {
            path: catalog.clone(),
            source,
        };
    })? {
        return Ok(None);
    }

    let records = read_artifact_roots(&catalog)?;
    let Some(record) = records
        .iter()
        .find(|record| return record.artifact_directory == artifact_directory)
    else {
        return Ok(None);
    };

    return verify_artifact_root(session, record, session_manifest_hash, source_event_hash)
        .map(Some);
}

fn verify_artifact_root(
    session: &Path,
    record: &ArtifactRootRecord,
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
) -> Result<PathBuf> {
    if record.session_manifest_hash != session_manifest_hash
        || record.source_prefix.is_none()
            && record.source_event_hash.as_deref() != source_event_hash
    {
        return Err(Error::invalid(
            "artifact-root catalog entry does not bind the canonical source roots",
        ));
    }

    if let Some(prefix) = &record.source_prefix {
        if record.source_event_hash.as_deref() != Some(prefix.event_hash.as_str()) {
            return Err(Error::invalid(
                "artifact catalog prefix hash differs from its source root",
            ));
        }

        validate_source_prefix(session, session_manifest_hash, prefix)?;
    }

    let relative = Path::new(&record.artifact_directory);
    normalized_relative_path(relative)?;
    let directory = session.join(relative);
    let manifest_path = directory.join(ARTIFACT_MANIFEST);
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;

    if blake3::hash(&manifest_bytes).to_hex().as_str() != record.manifest_hash {
        return Err(Error::invalid(
            "artifact manifest hash does not match its catalog root",
        ));
    }

    let manifest: ArtifactMemberEnvelope = serde_json::from_slice(&manifest_bytes)
        .map_err(|source| return json_error(manifest_path, source))?;

    if manifest.source_range != record.source_range {
        return Err(Error::invalid(
            "artifact catalog source range does not match its manifest",
        ));
    }

    if manifest.source_event_hash != record.source_event_hash {
        return Err(Error::invalid(
            "artifact catalog source event hash does not match its manifest",
        ));
    }

    verify_artifact_members(&directory, &manifest.members)?;
    return Ok(directory);
}

fn validate_source_prefix(
    session: &Path,
    manifest_hash: &str,
    prefix: &CanonicalBoundary,
) -> Result<()> {
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

    if cursor.header().session_manifest_hash != manifest_hash {
        return Err(Error::invalid(
            "artifact prefix has a different source manifest",
        ));
    }

    let mut output_bytes = 0_u64;

    while cursor.next_event_seq() < prefix.next_event_seq {
        let record = cursor
            .next_record()?
            .ok_or_else(|| return Error::invalid("artifact source prefix is unavailable"))?;

        if let Event::Output { data } = record.event {
            output_bytes = output_bytes
                .saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        }
    }

    cursor.validate_boundary(prefix.next_event_seq, &prefix.event_hash)?;

    if output_bytes != prefix.output_bytes {
        return Err(Error::invalid(
            "artifact prefix output byte count differs from canonical evidence",
        ));
    }
    return Ok(());
}

fn collect_member_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<fs::DirEntry> = Vec::new();

    for entry in fs::read_dir(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })? {
        let entry = entry.map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;
        entries.push(entry);
    }

    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        if metadata.file_type().is_symlink() {
            return Err(Error::invalid("artifact payload contains a symlink"));
        }

        if metadata.is_dir() {
            collect_member_paths(root, &path, paths)?;
        } else if metadata.is_file() {
            paths.push(
                path.strip_prefix(root)
                    .map_err(|source| {
                        return Error::invalid_with_source(
                            "artifact member escaped its root",
                            source,
                        );
                    })?
                    .to_path_buf(),
            );
        } else {
            return Err(Error::invalid(
                "artifact payload contains a device or special file",
            ));
        }
    }

    return Ok(());
}

pub(super) fn normalized_relative_path(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| return !matches!(component, Component::Normal(_)))
    {
        return Err(Error::invalid("artifact member path is not normalized"));
    }

    let normalized = path
        .to_str()
        .ok_or_else(|| return Error::invalid("artifact member path is not valid UTF-8"))?;

    if normalized.contains('\\') {
        return Err(Error::invalid(
            "artifact member path uses a non-portable separator",
        ));
    }
    return Ok(normalized.to_owned());
}

/// Hash a regular file without following a symbolic link.
///
/// Returns its exact byte length and BLAKE3 digest.
///
/// # Errors
///
/// Returns an error when the path is not a readable regular file.
pub fn hash_regular_file(path: &Path) -> Result<(u64, String)> {
    let (mut file, size) = open_regular_file_nofollow(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return Ok((size, hasher.finalize().to_hex().to_string()));
}

/// Open a regular artifact file without following symlinks in its path.
///
/// # Errors
/// Returns filesystem errors and rejects nonregular files.
pub fn open_regular_file_nofollow(path: &Path) -> Result<(File, u64)> {
    let parent = open_parent_directory(path, false)?;
    let leaf = safe_file_name(path)?;
    let fd = open_file_at(
        &parent,
        leaf,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
        Mode::empty(),
        path,
    )?;
    let stat = fstat(&fd).map_err(|error| return nix_filesystem(path, error))?;
    if !SFlag::from_bits_truncate(stat.st_mode).contains(SFlag::S_IFREG) || stat.st_size < 0 {
        return Err(Error::invalid("artifact member is not a regular file"));
    }

    let size = u64::try_from(stat.st_size).map_err(|source| {
        return Error::invalid_with_source("artifact member size is invalid", source);
    })?;

    return Ok((File::from(fd), size));
}

/// Media type of an artifact or bundle member, classified by its file name.
pub fn member_media_type(path: &str) -> &'static str {
    if path.ends_with(".jsonl") {
        return "application/x-ndjson";
    } else if path.ends_with(".json") {
        return "application/json";
    } else if path.ends_with(".zst") {
        return "application/zstd";
    } else if path.ends_with(".html") {
        return "text/html";
    } else if path.ends_with(".png") {
        return "image/png";
    } else if path.ends_with(".mkv") {
        return "video/x-matroska";
    } else if path.ends_with(".vt") {
        return "application/vnd.termtrace.vt";
    }

    return "text/plain";
}

/// Public schema identifier of a structured artifact or bundle member, when it has one.
pub fn member_schema_type(path: &str) -> Option<&'static str> {
    match Path::new(path).file_name()?.to_str()? {
        layout::MANIFEST => return Some("session.schema.json"),
        layout::COMPLETION => return Some("completion.schema.json"),
        STATE_INDEX => return Some("state-index-entry.schema.json"),
        "video-map.jsonl" => return Some("video-map-entry.schema.json"),
        "replay-endpoint.json" => return Some("replay-endpoint.schema.json"),
        "semantic-source-map.jsonl" => return Some("semantic-frame-span.schema.json"),
        _ => return None,
    }
}

fn member_relationship(path: &str) -> &'static str {
    match Path::new(path)
        .file_name()
        .and_then(|value| return value.to_str())
        .unwrap_or(path)
    {
        STATE_INDEX => return "semantic_state_index",
        "final.txt" | "final.vt" | "final.html" => return "semantic_final_state",
        "session.mkv" => return "native_capture_master",
        "final.png" => return "native_final_frame",
        "video-map.jsonl" => return "semantic_frame_mapping",
        "replay-endpoint.json" => return "canonical_replay_endpoint",
        "semantic-source.mkv" => return "semantic_rendering_master",
        "semantic-source-map.jsonl" => return "semantic_source_frame_mapping",
        "semantic-source-manifest.json" => return "semantic_source_manifest",
        _ => return "derivation_diagnostic",
    }
}
