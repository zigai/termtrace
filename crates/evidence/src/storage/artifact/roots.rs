use std::{
    ffi::OsStr,
    fs::File,
    io::{BufRead as _, BufReader, Error as IoError, ErrorKind, Write as _},
    path::Path,
};

use nix::fcntl::{Flock, FlockArg, OFlag};
use nix::sys::stat::Mode;
use serde::Serialize;
use termtrace_model::artifact::metadata::{ArtifactRootRecord, ArtifactSourceRange};
use termtrace_model::execution::event_log::CanonicalBoundary;
use uuid::Uuid;

use crate::storage::{Error, Result};

use super::{
    ARTIFACT_ROOT_FORMAT, ARTIFACT_ROOT_LOCK, ARTIFACT_ROOT_LOG, ARTIFACT_ROOT_VERSION, json_error,
    secure_fs::{
        open_directory_path, open_file_at, open_parent_directory, safe_file_name, sync_fd,
        validate_private_directory_fd, validate_private_regular_file_fd,
    },
    verification::normalized_relative_path,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    source_prefix: Option<&'record CanonicalBoundary>,
}

struct RootSource<'source> {
    hash: Option<&'source str>,
    range: Option<ArtifactSourceRange>,
    prefix: Option<&'source CanonicalBoundary>,
}

/// Append a derivation root to a session's locked, hash-chained artifact catalog.
///
/// # Errors
///
/// Returns an error when paths escape the session, a root already exists for the artifact
/// directory, or the record cannot be persisted durably.
pub fn append_artifact_root(
    session: &Path,
    artifact_directory: &Path,
    derivation_type: &str,
    manifest_bytes: &[u8],
    session_manifest_hash: &str,
    source_event_hash: Option<&str>,
    source_range: Option<ArtifactSourceRange>,
) -> Result<ArtifactRootRecord> {
    return append_root(
        session,
        artifact_directory,
        derivation_type,
        manifest_bytes,
        session_manifest_hash,
        RootSource {
            hash: source_event_hash,
            range: source_range,
            prefix: None,
        },
    );
}

/// Catalog an artifact against an explicit canonical prefix, including live sources.
///
/// # Errors
/// Returns path, catalog integrity, or durable publication failures.
pub fn append_artifact_prefix_root(
    session: &Path,
    artifact_directory: &Path,
    derivation_type: &str,
    manifest_bytes: &[u8],
    session_manifest_hash: &str,
    boundary: &CanonicalBoundary,
) -> Result<ArtifactRootRecord> {
    return append_root(
        session,
        artifact_directory,
        derivation_type,
        manifest_bytes,
        session_manifest_hash,
        RootSource {
            hash: Some(&boundary.event_hash),
            range: None,
            prefix: Some(boundary),
        },
    );
}

fn append_root(
    session: &Path,
    artifact_directory: &Path,
    derivation_type: &str,
    manifest_bytes: &[u8],
    session_manifest_hash: &str,
    source: RootSource<'_>,
) -> Result<ArtifactRootRecord> {
    let directory_name = artifact_directory
        .strip_prefix(session)
        .map_err(|_| return Error::invalid("artifact directory is outside its source session"))?;
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
        return Err(Error::invalid(
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
        source_event_hash: source.hash,
        source_range: source.range,
        source_prefix: source.prefix,
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
        source_event_hash: source.hash.map(str::to_owned),
        source_range: source.range,
        source_prefix: source.prefix.cloned(),
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

/// Read and validate an artifact-root catalog.
///
/// A missing catalog is treated as empty.
///
/// # Errors
///
/// Returns an error when the catalog is unsafe, unreadable, or violates its hash chain.
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
    return read_artifact_roots_from(&mut File::from(fd), path);
}

fn read_artifact_roots_from(file: &mut File, path: &Path) -> Result<Vec<ArtifactRootRecord>> {
    let reader = BufReader::new(file);
    let mut records: Vec<ArtifactRootRecord> = Vec::new();
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
            return Err(Error::invalid(
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
            source_prefix: record.source_prefix.as_ref(),
        };
        if record.hash != hash_artifact_root(&body, path)? {
            return Err(Error::invalid(
                "artifact-root catalog record hash does not match",
            ));
        }
        previous_hash = record.hash.clone();
        records.push(record);
    }
    return Ok(records);
}

fn hash_artifact_root(body: &ArtifactRootBody<'_>, path: &Path) -> Result<String> {
    let bytes =
        serde_json::to_vec(body).map_err(|source| return json_error(path.to_path_buf(), source))?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}
