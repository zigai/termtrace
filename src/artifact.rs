//! Owner-only, durable, no-replace artifact filesystem helpers.

use std::{
    collections::BTreeSet,
    fs::{DirBuilder, File, OpenOptions, Permissions},
    io::{BufRead as _, BufReader, BufWriter, Error as IoError, ErrorKind, Read as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
    result::Result as StdResult,
};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, RenameFlags, renameat2},
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
    let mut paths = Vec::new();
    collect_member_paths(directory, directory, &mut paths)?;
    paths.sort();
    let mut members = Vec::with_capacity(paths.len());
    for relative in paths {
        let normalized = normalized_relative_path(&relative)?;
        if normalized == "manifest.json" {
            continue;
        }
        let path = directory.join(&relative);
        let (byte_length, hash) = hash_regular_file(&path)?;
        members.push(ArtifactMember {
            media_type: member_media_type(&normalized).to_owned(),
            schema_type: member_schema_type(&normalized).map(str::to_owned),
            sensitivity: ArtifactSensitivity::SensitiveTerminalData,
            relationship: member_relationship(&normalized).to_owned(),
            path: normalized,
            byte_length,
            blake3: hash,
        });
    }
    return Ok(members);
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
    let lock_path = session.join(ARTIFACT_ROOT_LOCK);
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|source| {
            return Error::Filesystem {
                path: lock_path.clone(),
                source,
            };
        })?;
    sync_parent_directory(&lock_path)?;
    let lock = Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, error)| {
        return Error::Filesystem {
            path: lock_path,
            source: IoError::from_raw_os_error(error as i32),
        };
    })?;
    let catalog_path = session.join(ARTIFACT_ROOT_LOG);
    let records = read_artifact_roots(&catalog_path)?;
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
    let mut catalog = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&catalog_path)
        .map_err(|source| {
            return Error::Filesystem {
                path: catalog_path.clone(),
                source,
            };
        })?;
    catalog
        .write_all(&encoded)
        .and_then(|()| return catalog.sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: catalog_path.clone(),
                source,
            };
        })?;
    sync_parent_directory(&catalog_path)?;
    drop(lock);
    return Ok(record);
}

/// Read and validate the complete local artifact-root hash chain.
///
/// # Errors
///
/// Returns an error for malformed JSON, gaps, or hash disagreement.
pub fn read_artifact_roots(path: &Path) -> Result<Vec<ArtifactRootRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = BufReader::new(File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?);
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

fn collect_member_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs_err::read_dir(directory)
        .map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?
        .collect::<StdResult<Vec<_>, _>>()
        .map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;
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
            collect_member_paths(root, &path, paths)?;
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

fn hash_regular_file(path: &Path) -> Result<(u64, String)> {
    let metadata = fs_err::symlink_metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid_artifact("artifact member is not a regular file"));
    }
    let mut file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mut hasher = blake3::Hasher::new();
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
        hasher.update(buffer.get(..read).ok_or_else(|| {
            return invalid_artifact("artifact hashing buffer bounds are invalid");
        })?);
    }
    return Ok((metadata.len(), hasher.finalize().to_hex().to_string()));
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
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return sync_parent_directory(path);
}

pub(crate) fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    return secure_file(path).map(BufWriter::new);
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
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

pub(crate) fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
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
    return File::open(path)
        .and_then(|directory| return directory.sync_all())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        };
    })?;
    return sync_directory(parent);
}

pub(crate) fn commit_directory(session: &Path, staging: &Path, destination: &Path) -> Result<()> {
    let directory = File::open(session).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
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
    return sync_directory(session);
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

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
    fn member_collection_rejects_symlinks() {
        let temporary = TempDir::new().expect("temporary directory");
        let artifact = temporary.path().join("artifact");
        fs_err::create_dir(&artifact).expect("artifact directory");
        fs_err::write(temporary.path().join("outside"), b"secret").expect("outside file");
        symlink(temporary.path().join("outside"), artifact.join("linked"))
            .expect("payload symlink");
        assert!(collect_artifact_members(&artifact).is_err());
    }
}
