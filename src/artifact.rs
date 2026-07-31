//! Owner-only, durable, no-replace artifact filesystem helpers.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::File,
    io::{BufRead as _, BufReader, BufWriter, Error as IoError, ErrorKind, Read as _, Write as _},
    os::fd::{AsFd, OwnedFd},
    path::{Component, Path, PathBuf},
    thread,
};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, OFlag, OpenHow, RenameFlags, ResolveFlag, open, openat2, renameat2},
    sys::stat::{Mode, SFlag, fstat, mkdirat},
    unistd::{Uid, fsync},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result};

/// Relative append-only artifact-root catalog path.
pub const ARTIFACT_ROOT_LOG: &str = "artifact-roots.jsonl";
const ARTIFACT_ROOT_LOCK: &str = ".artifact-roots.lock";
const ARTIFACT_ROOT_FORMAT: &str = "termtrace-artifact-root";
const ARTIFACT_ROOT_VERSION: u32 = 1;

/// Sensitivity classification for one derived artifact member.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSensitivity {
    /// Member contains or may reveal terminal content.
    SensitiveTerminalData,
    /// Member contains only non-content operational metadata.
    OperationalMetadata,
}

/// Whole-file integrity metadata embedded in a derived manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMember {
    /// Normalized relative path below the artifact directory.
    pub path: String,
    /// Exact byte length.
    pub byte_length: u64,
    /// BLAKE3 of the complete file.
    pub blake3: String,
    /// Stable media type.
    pub media_type: String,
    /// Public schema identifier, when the file is structured data.
    pub schema_type: Option<String>,
    /// Sensitivity classification.
    pub sensitivity: ArtifactSensitivity,
    /// Relationship to the derivation.
    pub relationship: String,
}

/// Half-open canonical source range bound into an artifact catalog root.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSourceRange {
    /// Inclusive canonical monotonic start in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive canonical monotonic end in nanoseconds.
    pub end_monotonic_ns: u64,
}

/// One hash-chained post-completion artifact-root entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRootRecord {
    /// Stable record format identifier.
    pub format: String,
    /// Record schema version.
    pub version: u32,
    /// Contiguous zero-based catalog sequence.
    pub seq: u64,
    /// Prior catalog record hash, or 64 zeroes for the first entry.
    pub prev_hash: String,
    /// Hash of this record body and prior hash.
    pub hash: String,
    /// Unique derivation identifier.
    pub derivation_id: Uuid,
    /// Stable derivation family.
    pub derivation_type: String,
    /// Artifact directory relative to the canonical session.
    pub artifact_directory: String,
    /// BLAKE3 of the exact artifact manifest bytes.
    pub manifest_hash: String,
    /// Session-manifest root committed by the event header.
    pub session_manifest_hash: String,
    /// Source event boundary used by the derivation.
    pub source_event_hash: Option<String>,
    /// Selected half-open canonical source interval, if any.
    pub source_range: Option<ArtifactSourceRange>,
}

#[derive(Serialize)]
struct ArtifactRootBody<'record> {
    format: &'record str,
    version: u32,
    seq: u64,
    prev_hash: &'record str,
    derivation_id: Uuid,
    derivation_type: &'record str,
    artifact_directory: &'record str,
    manifest_hash: &'record str,
    session_manifest_hash: &'record str,
    source_event_hash: Option<&'record str>,
    source_range: Option<ArtifactSourceRange>,
}

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

/// Append a manifested derivation root to the separate post-completion chain.
///
/// # Errors
///
/// Returns an error when the catalog is corrupt, locking fails, or the new
/// record cannot be durably appended.
pub fn append_artifact_root(
    session: &Path,
    artifact_directory: &Path,
    derivation_type: &str,
    manifest_bytes: &[u8],
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
    source_range: Option<ArtifactSourceRange>,
) -> Result<ArtifactRootRecord> {
    let directory_name = artifact_directory
        .strip_prefix(session)
        .map_err(|_| return invalid_artifact("artifact directory is outside its source session"))?;
    let directory_name = normalized_relative_path(directory_name)?;
    let session_fd = open_directory_path(session, false)?;
    validate_private_directory_fd(&session_fd, session)?;
    let lock_path = session.join(ARTIFACT_ROOT_LOCK);
    let lock_fd = open_file_at(
        &session_fd,
        OsStr::new(ARTIFACT_ROOT_LOCK),
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        &lock_path,
    )?;
    validate_private_regular_file_fd(&lock_fd, &lock_path)?;
    sync_fd(&session_fd, session)?;
    let lock =
        Flock::lock(File::from(lock_fd), FlockArg::LockExclusive).map_err(|(_, error)| {
            return Error::Filesystem {
                path: lock_path,
                source: IoError::from_raw_os_error(error as i32),
            };
        })?;
    let catalog_path = session.join(ARTIFACT_ROOT_LOG);
    let catalog_fd = open_file_at(
        &session_fd,
        OsStr::new(ARTIFACT_ROOT_LOG),
        OFlag::O_RDWR | OFlag::O_APPEND | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        &catalog_path,
    )?;
    validate_private_regular_file_fd(&catalog_fd, &catalog_path)?;
    sync_fd(&session_fd, session)?;
    let mut catalog = File::from(catalog_fd);
    let records = read_artifact_roots_from(&mut catalog, &catalog_path)?;
    if records
        .iter()
        .any(|record| return record.artifact_directory == directory_name)
    {
        return Err(invalid_artifact(
            "artifact directory already has a catalog root",
        ));
    }
    let seq = u64::try_from(records.len()).unwrap_or(u64::MAX);
    let prev_hash = records.last().map_or_else(
        || return "0".repeat(64),
        |record| return record.hash.clone(),
    );
    let derivation_id = Uuid::now_v7();
    let manifest_hash = blake3::hash(manifest_bytes).to_hex().to_string();
    let body = ArtifactRootBody {
        format: ARTIFACT_ROOT_FORMAT,
        version: ARTIFACT_ROOT_VERSION,
        seq,
        prev_hash: &prev_hash,
        derivation_id,
        derivation_type,
        artifact_directory: &directory_name,
        manifest_hash: &manifest_hash,
        session_manifest_hash,
        source_event_hash,
        source_range,
    };
    let hash = hash_artifact_root(&body, &catalog_path)?;
    let record = ArtifactRootRecord {
        format: ARTIFACT_ROOT_FORMAT.to_owned(),
        version: ARTIFACT_ROOT_VERSION,
        seq,
        prev_hash,
        hash,
        derivation_id,
        derivation_type: derivation_type.to_owned(),
        artifact_directory: directory_name,
        manifest_hash,
        session_manifest_hash: session_manifest_hash.to_owned(),
        source_event_hash: source_event_hash.map(str::to_owned),
        source_range,
    };
    let mut encoded = serde_json::to_vec(&record)
        .map_err(|source| return json_error(catalog_path.clone(), source))?;
    encoded.push(b'\n');
    catalog
        .write_all(&encoded)
        .and_then(|()| return catalog.sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: catalog_path.clone(),
                source,
            };
        })?;
    sync_fd(&session_fd, session)?;
    drop(lock);
    return Ok(record);
}

/// Read and validate the complete local artifact-root hash chain.
///
/// # Errors
///
/// Returns an error for malformed JSON, gaps, or hash disagreement.
pub fn read_artifact_roots(path: &Path) -> Result<Vec<ArtifactRootRecord>> {
    let parent = open_parent_directory(path, false)?;
    let leaf = safe_file_name(path)?;
    let fd = match open_file_at(
        &parent,
        leaf,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
        path,
    ) {
        Ok(fd) => fd,
        Err(Error::Filesystem { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    validate_private_regular_file_fd(&fd, path)?;
    let mut file = File::from(fd);
    return read_artifact_roots_from(&mut file, path);
}

fn read_artifact_roots_from(file: &mut File, path: &Path) -> Result<Vec<ArtifactRootRecord>> {
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut previous_hash = "0".repeat(64);
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        let record: ArtifactRootRecord = serde_json::from_str(&line)
            .map_err(|source| return json_error(path.to_path_buf(), source))?;
        let seq = u64::try_from(index).unwrap_or(u64::MAX);
        if record.format != ARTIFACT_ROOT_FORMAT
            || record.version != ARTIFACT_ROOT_VERSION
            || record.seq != seq
            || record.prev_hash != previous_hash
        {
            return Err(invalid_artifact(
                "artifact-root catalog format, sequence, or prior hash is invalid",
            ));
        }
        let body = ArtifactRootBody {
            format: &record.format,
            version: record.version,
            seq: record.seq,
            prev_hash: &record.prev_hash,
            derivation_id: record.derivation_id,
            derivation_type: &record.derivation_type,
            artifact_directory: &record.artifact_directory,
            manifest_hash: &record.manifest_hash,
            session_manifest_hash: &record.session_manifest_hash,
            source_event_hash: record.source_event_hash.as_deref(),
            source_range: record.source_range,
        };
        if record.hash != hash_artifact_root(&body, path)? {
            return Err(invalid_artifact(
                "artifact-root catalog record hash does not match",
            ));
        }
        previous_hash = record.hash.clone();
        records.push(record);
    }
    return Ok(records);
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

fn normalized_relative_path(path: &Path) -> Result<String> {
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

fn hash_artifact_root(body: &ArtifactRootBody<'_>, path: &Path) -> Result<String> {
    let bytes =
        serde_json::to_vec(body).map_err(|source| return json_error(path.to_path_buf(), source))?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn invalid_artifact(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    let parent = open_parent_directory(path, true)?;
    let leaf = safe_file_name(path)?;
    mkdirat(&parent, leaf, Mode::from_bits_truncate(0o700)).map_err(|error| {
        return nix_filesystem(path, error);
    })?;
    let directory = open_directory_at(&parent, leaf, path)?;
    validate_private_directory_fd(&directory, path)?;
    return sync_fd(
        &parent,
        path.parent().unwrap_or_else(|| return Path::new(".")),
    );
}

pub(crate) fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    return secure_file(path).map(BufWriter::new);
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
    let parent = open_parent_directory(path, true)?;
    let leaf = safe_file_name(path)?;
    let file = open_file_at(
        &parent,
        leaf,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        path,
    )?;
    validate_private_regular_file_fd(&file, path)?;
    sync_fd(
        &parent,
        path.parent().unwrap_or_else(|| return Path::new(".")),
    )?;
    return Ok(File::from(file));
}

fn open_parent_directory(path: &Path, create_missing: bool) -> Result<OwnedFd> {
    let Some(parent) = path.parent() else {
        return Err(Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        });
    };
    return open_directory_path(parent, create_missing);
}

fn open_directory_path(path: &Path, create_missing: bool) -> Result<OwnedFd> {
    let starting_path = if path.is_absolute() { "/" } else { "." };
    let mut directory = open(
        starting_path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| return nix_filesystem(Path::new(starting_path), error))?;
    let mut traversed = PathBuf::from(starting_path);
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::Filesystem {
                    path: path.to_path_buf(),
                    source: IoError::new(
                        ErrorKind::InvalidInput,
                        "artifact path contains an unsupported parent or prefix component",
                    ),
                });
            }
        };
        traversed.push(name);
        let mut created_or_raced = false;
        let next = match open_directory_at(&directory, name, &traversed) {
            Ok(next) => next,
            Err(Error::Filesystem { source, .. })
                if create_missing && source.kind() == ErrorKind::NotFound =>
            {
                created_or_raced = true;
                match mkdirat(&directory, name, Mode::from_bits_truncate(0o700)) {
                    Ok(()) | Err(Errno::EEXIST) => {}
                    Err(error) => return Err(nix_filesystem(&traversed, error)),
                }
                let next = open_directory_at(&directory, name, &traversed)?;
                sync_fd(
                    &directory,
                    traversed.parent().unwrap_or_else(|| return Path::new(".")),
                )?;
                next
            }
            Err(error) => return Err(error),
        };
        if created_or_raced {
            validate_private_directory_fd(&next, &traversed)?;
        }
        directory = next;
    }
    return Ok(directory);
}

fn open_directory_at(parent: &OwnedFd, name: &OsStr, path: &Path) -> Result<OwnedFd> {
    return openat2(
        parent,
        name,
        OpenHow::new()
            .flags(OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW)
            .resolve(ResolveFlag::RESOLVE_BENEATH | ResolveFlag::RESOLVE_NO_SYMLINKS),
    )
    .map_err(|error| return nix_filesystem(path, error));
}

fn open_file_at(
    parent: &OwnedFd,
    name: &OsStr,
    flags: OFlag,
    mode: Mode,
    path: &Path,
) -> Result<OwnedFd> {
    return openat2(
        parent,
        name,
        OpenHow::new()
            .flags(flags)
            .mode(mode)
            .resolve(ResolveFlag::RESOLVE_BENEATH | ResolveFlag::RESOLVE_NO_SYMLINKS),
    )
    .map_err(|error| return nix_filesystem(path, error));
}

fn safe_file_name(path: &Path) -> Result<&OsStr> {
    return path.file_name().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "artifact path has no file name"),
        };
    });
}

fn validate_private_directory_fd(directory: &OwnedFd, path: &Path) -> Result<()> {
    let stat = fstat(directory).map_err(|error| return nix_filesystem(path, error))?;
    let kind = SFlag::from_bits_truncate(stat.st_mode);
    let mode = stat.st_mode & 0o777;
    if !kind.contains(SFlag::S_IFDIR) || stat.st_uid != Uid::effective().as_raw() || mode != 0o700 {
        return Err(Error::InsecureArtifactDirectory {
            path: path.to_path_buf(),
            owner_uid: stat.st_uid,
            mode,
        });
    }
    return Ok(());
}

fn validate_private_regular_file_fd(file: &OwnedFd, path: &Path) -> Result<()> {
    let stat = fstat(file).map_err(|error| return nix_filesystem(path, error))?;
    let kind = SFlag::from_bits_truncate(stat.st_mode);
    let mode = stat.st_mode & 0o777;
    if !kind.contains(SFlag::S_IFREG) || stat.st_uid != Uid::effective().as_raw() || mode != 0o600 {
        return Err(Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(
                ErrorKind::PermissionDenied,
                format!(
                    "artifact file must be owner-only regular file (owner={}, mode={mode:o})",
                    stat.st_uid
                ),
            ),
        });
    }
    return Ok(());
}

fn sync_fd(fd: &impl AsFd, path: &Path) -> Result<()> {
    return fsync(fd).map_err(|error| return nix_filesystem(path, error));
}

fn nix_filesystem(path: &Path, error: Errno) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source: IoError::from_raw_os_error(error as i32),
    };
}

pub(crate) fn finish_writer(mut writer: BufWriter<File>, path: &Path) -> Result<()> {
    writer
        .flush()
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return sync_parent_directory(path);
}

/// Create, flush, and parent-sync one owner-only no-replace artifact file.
///
/// # Errors
///
/// Returns an error when the destination exists or any write/sync operation fails.
pub fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut writer = secure_writer(path)?;
    writer
        .write_all(bytes)
        .and_then(|()| return writer.flush())
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return sync_parent_directory(path);
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    let directory = open_directory_path(path, false)?;
    return sync_fd(&directory, path);
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        };
    })?;
    let directory = open_directory_path(parent, false)?;
    return sync_fd(&directory, parent);
}

pub(crate) fn commit_directory(session: &Path, staging: &Path, destination: &Path) -> Result<()> {
    if staging.parent() != Some(session) || destination.parent() != Some(session) {
        return Err(invalid_artifact(
            "staging and destination directories must be direct session children",
        ));
    }
    let directory = open_directory_path(session, false)?;
    let staging_name = staging.file_name().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "staging directory has no basename".to_owned(),
        };
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "artifact directory has no basename".to_owned(),
        };
    })?;
    renameat2(
        &directory,
        staging_name,
        &directory,
        destination_name,
        RenameFlags::RENAME_NOREPLACE,
    )
    .map_err(|error| {
        if error == Errno::EEXIST {
            return Error::ArtifactExists {
                path: destination.to_path_buf(),
            };
        } else {
            return Error::Filesystem {
                path: destination.to_path_buf(),
                source: IoError::from_raw_os_error(error as i32),
            };
        }
    })?;
    return sync_fd(&directory, session);
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write as _,
        os::unix::fs::{PermissionsExt as _, symlink},
    };

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn member_verification_rejects_extra_and_changed_payloads() {
        let temporary = TempDir::new().expect("temporary directory");
        let artifact = temporary.path().join("artifact");
        fs_err::create_dir(&artifact).expect("artifact directory");
        fs_err::write(artifact.join("final.txt"), b"original").expect("artifact member");
        let members = collect_artifact_members(&artifact).expect("collect members");
        verify_artifact_members(&artifact, &members).expect("members should verify");

        fs_err::write(artifact.join("extra.log"), b"unmanifested").expect("extra member");
        assert!(verify_artifact_members(&artifact, &members).is_err());
        fs_err::remove_file(artifact.join("extra.log")).expect("remove extra member");
        fs_err::write(artifact.join("final.txt"), b"changed").expect("change member");
        assert!(verify_artifact_members(&artifact, &members).is_err());
    }

    #[test]
    fn parallel_member_hashing_preserves_canonical_path_order() {
        let temporary = TempDir::new().expect("temporary directory");
        let artifact = temporary.path().join("artifact");
        fs_err::create_dir(&artifact).expect("artifact directory");
        for name in ["z.txt", "a.txt", "m.txt", "b.txt", "y.txt", "c.txt"] {
            fs_err::write(artifact.join(name), name.as_bytes()).expect("artifact member");
        }

        let members = collect_artifact_members(&artifact).expect("collect members");
        assert_eq!(
            members
                .iter()
                .map(|member| return member.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt", "c.txt", "m.txt", "y.txt", "z.txt"]
        );
        verify_artifact_members(&artifact, &members).expect("ordered members should verify");
    }

    #[test]
    fn member_collection_rejects_symlinks() {
        let temporary = TempDir::new().expect("temporary directory");
        let artifact = temporary.path().join("artifact");
        fs_err::create_dir(&artifact).expect("artifact directory");
        fs_err::write(temporary.path().join("outside"), b"secret").expect("outside file");
        symlink(temporary.path().join("outside"), artifact.join("linked"))
            .expect("payload symlink");
        assert!(collect_artifact_members(&artifact).is_err());
    }

    #[test]
    fn secure_outputs_create_missing_owner_only_parent_directories() {
        let temporary = TempDir::new().expect("temporary directory");
        let nested = temporary.path().join("missing/private/artifact");
        create_private_directory(&nested).expect("nested artifact directory");
        write_secure(&nested.join("report.json"), b"{}\n").expect("nested artifact file");
        for path in [
            temporary.path().join("missing"),
            temporary.path().join("missing/private"),
            nested,
        ] {
            let mode = fs_err::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn secure_outputs_do_not_follow_a_missing_parent_symlink() {
        let temporary = TempDir::new().expect("temporary directory");
        let outside = temporary.path().join("outside");
        fs_err::create_dir(&outside).expect("outside directory");
        let link = temporary.path().join("linked-parent");
        symlink(&outside, &link).expect("parent symlink");

        assert!(write_secure(&link.join("nested/report.json"), b"{}\n").is_err());
        assert!(!outside.join("nested").exists());
    }

    #[test]
    fn secure_outputs_reject_symlinks_before_existing_parent_components() {
        let temporary = TempDir::new().expect("temporary directory");
        let outside = temporary.path().join("outside");
        fs_err::create_dir(&outside).expect("outside directory");
        fs_err::create_dir(outside.join("existing")).expect("existing outside descendant");
        let link = temporary.path().join("linked-parent");
        symlink(&outside, &link).expect("parent symlink");

        assert!(write_secure(&link.join("existing/nested/report.json"), b"{}\n").is_err());
        assert!(!outside.join("existing/nested").exists());
    }

    #[test]
    fn retained_parent_descriptor_prevents_create_redirection() {
        let temporary = TempDir::new().expect("temporary directory");
        let parent = temporary.path().join("parent");
        let moved = temporary.path().join("moved-parent");
        let outside = temporary.path().join("outside");
        create_private_directory(&parent).expect("private parent");
        create_private_directory(&outside).expect("outside directory");

        let target = parent.join("artifact.json");
        let parent_fd = open_parent_directory(&target, false).expect("retain parent descriptor");
        fs_err::rename(&parent, &moved).expect("replace pathname after validation");
        symlink(&outside, &parent).expect("redirect original pathname");

        let fd = open_file_at(
            &parent_fd,
            OsStr::new("artifact.json"),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
            &target,
        )
        .expect("descriptor-relative create");
        File::from(fd).write_all(b"safe").expect("write artifact");

        assert_eq!(
            fs_err::read(moved.join("artifact.json")).expect("bound output"),
            b"safe"
        );
        assert!(!outside.join("artifact.json").exists());
    }

    #[test]
    fn artifact_catalog_rejects_session_lock_and_catalog_symlinks() {
        let temporary = TempDir::new().expect("temporary directory");
        let outside = temporary.path().join("outside");
        create_private_directory(&outside).expect("outside directory");

        let real_session = temporary.path().join("real-session");
        create_private_directory(&real_session).expect("real session");
        let linked_session = temporary.path().join("linked-session");
        symlink(&real_session, &linked_session).expect("session symlink");
        let error = append_artifact_root(
            &linked_session,
            &linked_session.join("derived"),
            "semantic_index",
            b"{}",
            &"0".repeat(64),
            None,
            None,
        )
        .expect_err("session symlink must be rejected");
        assert!(matches!(error, Error::Filesystem { .. }));

        let lock_session = temporary.path().join("lock-session");
        create_private_directory(&lock_session).expect("lock session");
        let outside_lock = outside.join("lock");
        fs_err::write(&outside_lock, b"sentinel").expect("outside lock");
        symlink(&outside_lock, lock_session.join(ARTIFACT_ROOT_LOCK)).expect("lock symlink");
        assert!(
            append_artifact_root(
                &lock_session,
                &lock_session.join("derived"),
                "semantic_index",
                b"{}",
                &"0".repeat(64),
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(
            fs_err::read(&outside_lock).expect("unchanged lock"),
            b"sentinel"
        );

        let catalog_session = temporary.path().join("catalog-session");
        create_private_directory(&catalog_session).expect("catalog session");
        let outside_catalog = outside.join("catalog");
        fs_err::write(&outside_catalog, b"sentinel").expect("outside catalog");
        symlink(&outside_catalog, catalog_session.join(ARTIFACT_ROOT_LOG))
            .expect("catalog symlink");
        assert!(
            append_artifact_root(
                &catalog_session,
                &catalog_session.join("derived"),
                "semantic_index",
                b"{}",
                &"0".repeat(64),
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(
            fs_err::read(&outside_catalog).expect("unchanged catalog"),
            b"sentinel"
        );
    }
}
