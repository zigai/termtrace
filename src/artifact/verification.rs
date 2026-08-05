use super::*;

#[derive(Deserialize)]
struct ArtifactMemberEnvelope {
    members: Vec<ArtifactMember>,
    source_range: Option<ArtifactSourceRange>,
}

/// Hash every regular payload file below an artifact staging directory.
///
/// `manifest.json` is excluded to avoid a circular digest. Symlinks, devices,
/// non-UTF-8 paths, and path traversal are rejected.
///
/// # Errors
///
/// Returns an error when traversal or hashing fails, or a member is unsafe.
pub fn collect_artifact_members(directory: &Path) -> Result<Vec<ArtifactMember>> {
    return collect_artifact_members_bounded(directory, usize::MAX);
}

pub(crate) fn collect_artifact_members_bounded(
    directory: &Path,
    maximum_tree_entries: usize,
) -> Result<Vec<ArtifactMember>> {
    let mut paths = Vec::new();
    let mut tree_entries = 0_usize;
    collect_member_paths(
        directory,
        directory,
        &mut paths,
        &mut tree_entries,
        maximum_tree_entries,
    )?;
    paths.sort();
    let mut payloads = Vec::with_capacity(paths.len());
    for relative in paths {
        let normalized = normalized_relative_path(&relative)?;
        if normalized == "manifest.json" {
            continue;
        }
        payloads.push((directory.join(relative), normalized));
    }
    let worker_count = available_workers(payloads.len());
    let mut indexed = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            let payloads = &payloads;
            handles.push(
                scope.spawn(move || -> Result<Vec<(usize, ArtifactMember)>> {
                    let mut members = Vec::new();
                    for index in (worker..payloads.len()).step_by(worker_count) {
                        let (path, normalized) = payloads.get(index).ok_or_else(|| {
                            return invalid_artifact("artifact hashing index is outside payloads");
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
        let mut indexed = Vec::with_capacity(payloads.len());
        for handle in handles {
            let mut members = handle
                .join()
                .map_err(|_panic| return invalid_artifact("artifact hashing worker panicked"))??;
            indexed.append(&mut members);
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
    let available = thread::available_parallelism().map_or(1, usize::from);
    return available.min(4).min(item_count.max(1));
}

/// Verify every manifested member and reject unmanifested payload files.
///
/// # Errors
///
/// Returns an error for missing, altered, unsafe, duplicate, or extra members.
pub fn verify_artifact_members(directory: &Path, expected: &[ArtifactMember]) -> Result<()> {
    let actual = collect_artifact_members(directory)?;
    let expected_paths = expected
        .iter()
        .map(|member| return member.path.as_str())
        .collect::<BTreeSet<_>>();
    if expected_paths.len() != expected.len() {
        return Err(invalid_artifact(
            "artifact manifest contains duplicate member paths",
        ));
    }
    if actual != expected {
        return Err(invalid_artifact(
            "artifact payload members do not match their whole-file manifest",
        ));
    }
    return Ok(());
}

/// Verify every locally cataloged artifact against its manifest and source roots.
///
/// This establishes local payload/member, source-binding, and catalog-order
/// consistency. It does not establish authenticity without a trusted detached
/// catalog checkpoint.
///
/// # Errors
///
/// Returns an error when any catalog, manifest, member, or source binding fails.
pub fn verify_session_artifacts(
    session: &Path,
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
) -> Result<u64> {
    let records = read_artifact_roots(&session.join(ARTIFACT_ROOT_LOG))?;
    for record in &records {
        if record.session_manifest_hash != session_manifest_hash
            || record.source_event_hash.as_deref() != source_event_hash
        {
            return Err(invalid_artifact(
                "artifact-root catalog entry does not bind the canonical source roots",
            ));
        }
        let relative = Path::new(&record.artifact_directory);
        let normalized = normalized_relative_path(relative)?;
        if normalized != record.artifact_directory {
            return Err(invalid_artifact(
                "artifact-root directory path is not normalized",
            ));
        }
        let directory = session.join(relative);
        let manifest_path = directory.join("manifest.json");
        let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
            return Error::Filesystem {
                path: manifest_path.clone(),
                source,
            };
        })?;
        if blake3::hash(&manifest_bytes).to_hex().as_str() != record.manifest_hash {
            return Err(invalid_artifact(
                "artifact manifest hash does not match its catalog root",
            ));
        }
        let manifest: ArtifactMemberEnvelope = serde_json::from_slice(&manifest_bytes)
            .map_err(|source| return json_error(manifest_path, source))?;
        if manifest.source_range != record.source_range {
            return Err(invalid_artifact(
                "artifact catalog source range does not match its manifest",
            ));
        }
        verify_artifact_members(&directory, &manifest.members)?;
    }
    return Ok(u64::try_from(records.len()).unwrap_or(u64::MAX));
}

fn collect_member_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    tree_entries: &mut usize,
    maximum_tree_entries: usize,
) -> Result<()> {
    let mut entries = Vec::new();
    for entry in fs_err::read_dir(directory).map_err(|source| {
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
        *tree_entries = tree_entries.saturating_add(1);
        if *tree_entries > maximum_tree_entries {
            return Err(invalid_artifact(format!(
                "artifact tree exceeds {maximum_tree_entries} entry limit"
            )));
        }
        entries.push(entry);
    }
    entries.sort_by_key(fs_err::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs_err::symlink_metadata(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_artifact("artifact payload contains a symlink"));
        }
        if metadata.is_dir() {
            collect_member_paths(root, &path, paths, tree_entries, maximum_tree_entries)?;
        } else if metadata.is_file() {
            paths.push(
                path.strip_prefix(root)
                    .map_err(|_| return invalid_artifact("artifact member escaped its root"))?
                    .to_path_buf(),
            );
        } else {
            return Err(invalid_artifact(
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
        return Err(invalid_artifact("artifact member path is not normalized"));
    }
    let normalized = path.to_str().ok_or_else(|| {
        return invalid_artifact("artifact member path is not valid UTF-8");
    })?;
    if normalized.contains('\\') {
        return Err(invalid_artifact(
            "artifact member path uses a non-portable separator",
        ));
    }
    return Ok(normalized.to_owned());
}

pub(crate) fn hash_regular_file(path: &Path) -> Result<(u64, String)> {
    let (mut file, _size) = open_regular_file_nofollow(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut byte_length = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        if read == 0 {
            break;
        }
        byte_length = byte_length
            .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
            .ok_or_else(|| return invalid_artifact("artifact member byte length overflow"))?;
        hasher.update(buffer.get(..read).ok_or_else(|| {
            return invalid_artifact("artifact hashing buffer bounds are invalid");
        })?);
    }
    return Ok((byte_length, hasher.finalize().to_hex().to_string()));
}

pub(crate) fn open_regular_file_nofollow(path: &Path) -> Result<(File, u64)> {
    let parent = open_parent_directory(path, false)?;
    let leaf = safe_file_name(path)?;
    let fd = open_file_at(
        &parent,
        leaf,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
        path,
    )?;
    let stat = fstat(&fd).map_err(|error| return nix_filesystem(path, error))?;
    if !SFlag::from_bits_truncate(stat.st_mode).contains(SFlag::S_IFREG) || stat.st_size < 0 {
        return Err(invalid_artifact("artifact member is not a regular file"));
    }
    let size = u64::try_from(stat.st_size)
        .map_err(|_error| return invalid_artifact("artifact member size is invalid"))?;
    return Ok((File::from(fd), size));
}

fn member_media_type(path: &str) -> &'static str {
    if path.ends_with(".jsonl") {
        return "application/x-ndjson";
    }
    if path.ends_with(".json") {
        return "application/json";
    }
    if path.ends_with(".zst") {
        return "application/zstd";
    }
    if path.ends_with(".html") {
        return "text/html";
    }
    if path.ends_with(".png") {
        return "image/png";
    }
    if path.ends_with(".mkv") {
        return "video/x-matroska";
    }
    if path.ends_with(".vt") {
        return "application/vnd.termtrace.vt";
    }
    return "text/plain";
}

fn member_schema_type(path: &str) -> Option<&'static str> {
    let filename = Path::new(path).file_name()?.to_str()?;
    return match filename {
        "state-index.jsonl" => Some("state-index-entry.schema.json"),
        "video-map.jsonl" => Some("video-map-entry.schema.json"),
        "semantic-source-map.jsonl" => Some("semantic-frame-span.schema.json"),
        _ => None,
    };
}

fn member_relationship(path: &str) -> &'static str {
    let filename = Path::new(path)
        .file_name()
        .and_then(|value| return value.to_str())
        .unwrap_or(path);
    return match filename {
        "states.jsonl.zst" => "semantic_state_stream",
        "state-index.jsonl" => "semantic_state_index",
        "final.txt" | "final.vt" | "final.html" => "semantic_final_state",
        "session.mkv" => "native_capture_master",
        "final.png" => "native_final_frame",
        "video-map.jsonl" => "semantic_frame_mapping",
        "semantic-source.mkv" => "semantic_rendering_master",
        "semantic-source-map.jsonl" => "semantic_source_frame_mapping",
        "semantic-source-manifest.json" => "semantic_source_manifest",
        _ => "derivation_diagnostic",
    };
}
