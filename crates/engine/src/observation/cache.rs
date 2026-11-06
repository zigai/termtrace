use super::observation_error;
use crate::replay::{
    semantic_state_hash,
    trust::{self, ensure_private_cache_directory, ValidatedSession},
    EndpointInspection,
};
use crate::{Error, Result};
use nix::libc::O_NOFOLLOW;
use std::{
    fs::OpenOptions,
    io::{ErrorKind, Read as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::SystemTime,
};
use termtrace_evidence::artifact::{sync_directory, write_secure};
use termtrace_model::{
    execution::layout::SessionLayout,
    execution::session::SessionCompletion,
    terminal::observation::{
        ReplayCachePruneReport, ReplayCacheStatus, ReplayCheckpoint, StatePoint,
    },
    terminal::replay::{StateInspection, GHOSTTY_REVISION, STATE_FORMAT_VERSION},
    terminal::state::SemanticState,
};
use zstd::stream::{encode_all, read::Decoder};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CachedSemanticState {
    pub state_id: u64,
    pub event_seq: Option<u64>,
    pub event_byte_offset: Option<u64>,
    pub monotonic_ns: Option<u64>,
    pub state_hash: String,
    pub presented: bool,
    pub state: SemanticState,
}

const REPLAY_CHECKPOINT_VERSION: u32 = 4;
const MAX_REPLAY_CACHE_ENTRIES: u64 = 32;
const MAX_REPLAY_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REPLAY_CHECKPOINT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REPLAY_CHECKPOINT_DECOMPRESSED_BYTES: u64 = 128 * 1024 * 1024;

pub(super) fn load_checkpoint(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
) -> Result<Option<CachedSemanticState>> {
    let path = checkpoint_path(session, point, completion)?;
    match fs_err::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Filesystem { path, source });
        }
    }

    let validate = (|| -> Result<CachedSemanticState> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
        let metadata = file.metadata().map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(observation_error(
                "replay checkpoint must be a regular owner-only file",
            ));
        }

        let compressed_bytes = metadata.len();
        if compressed_bytes > MAX_REPLAY_CHECKPOINT_BYTES {
            return Err(observation_error(
                "replay checkpoint exceeds its compressed size limit",
            ));
        }

        let decoder = Decoder::new(file).map_err(|error| {
            return observation_error(format!("could not decompress replay checkpoint: {error}"));
        })?;

        let mut bytes = Vec::new();
        decoder
            .take(MAX_REPLAY_CHECKPOINT_DECOMPRESSED_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;

        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REPLAY_CHECKPOINT_DECOMPRESSED_BYTES
        {
            return Err(observation_error(
                "replay checkpoint exceeds its decompressed size limit",
            ));
        }

        let checkpoint: ReplayCheckpoint = trust::decode(session, &bytes)?;
        let completion_bytes = read_completion_bytes(session)?;

        let state_bytes = serde_json::to_vec(&checkpoint.state).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<checkpoint-state>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let state_blake3 = blake3::hash(&state_bytes).to_hex().to_string();
        let semantic_hash = semantic_state_hash(&checkpoint.state)?;

        if checkpoint.version != REPLAY_CHECKPOINT_VERSION
            || checkpoint.state_format_version != STATE_FORMAT_VERSION
            || checkpoint.ghostty_revision != GHOSTTY_REVISION
            || checkpoint.state_point != *point
            || checkpoint.final_event_hash != completion.final_event_hash
            || checkpoint.completion_blake3 != blake3::hash(&completion_bytes).to_hex().to_string()
            || checkpoint.state_blake3 != state_blake3
            || checkpoint.state_hash != semantic_hash
        {
            return Err(observation_error(
                "replay checkpoint does not match exact canonical source bytes",
            ));
        }

        // Recorder endpoint snapshots can be authenticated yet lack canonical
        // source positions. A late snapshot must not change completed observation
        // facts after a concurrent replay/export. Reuse the existing canonical
        // replay fallback; genuine initial/empty absence is established there,
        // never filled with invented positions or accepted as cache authority.
        if checkpoint.event_seq.is_none() || checkpoint.monotonic_ns.is_none() {
            return Err(observation_error(
                "replay checkpoint lacks canonical source-position provenance",
            ));
        }
        return Ok(CachedSemanticState {
            state_id: checkpoint.state_id,
            event_seq: checkpoint.event_seq,
            event_byte_offset: checkpoint.event_byte_offset,
            monotonic_ns: checkpoint.monotonic_ns,
            state_hash: checkpoint.state_hash,
            presented: checkpoint.presented,
            state: checkpoint.state,
        });
    })();

    return match validate {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "rejected replay checkpoint");

            if let Err(remove_error) = fs_err::remove_file(&path) {
                tracing::warn!(path = %path.display(), %remove_error, "could not remove rejected replay checkpoint");
            } else if let Some(directory) = path.parent()
                && let Err(sync_error) = sync_directory(directory)
            {
                tracing::warn!(path = %directory.display(), %sync_error, "could not sync replay-cache cleanup");
            }

            Ok(None)
        }
    };
}

pub(super) fn publish_checkpoint(
    validated: &ValidatedSession,
    point: &StatePoint,
    inspection: &StateInspection,
) -> Result<()> {
    let state = CachedSemanticState {
        state_id: inspection.record.state_id,
        event_seq: inspection.record.event_seq,
        event_byte_offset: inspection.record.event_byte_offset,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        state_hash: inspection.record.state_hash.clone(),
        presented: inspection.record.presented,
        state: inspection.state.clone(),
    };

    return publish_cached_checkpoint(validated, point, &state);
}

pub(crate) fn publish_endpoint_checkpoint(
    validated: &ValidatedSession,
    point: &StatePoint,
    inspection: &EndpointInspection,
) -> Result<()> {
    let state = CachedSemanticState {
        state_id: inspection.state_id,
        event_seq: inspection.event_seq,
        event_byte_offset: inspection.event_byte_offset,
        monotonic_ns: inspection.monotonic_ns,
        state_hash: inspection.state_hash.clone(),
        presented: inspection.presented,
        state: inspection.state.clone(),
    };

    return publish_cached_checkpoint(validated, point, &state);
}

fn publish_cached_checkpoint(
    validated: &ValidatedSession,
    point: &StatePoint,
    state: &CachedSemanticState,
) -> Result<()> {
    let session = validated.root();
    let completion = validated.completion();
    let path = checkpoint_path(session, point, completion)?;

    if path.exists() {
        return Ok(());
    }

    let directory = path.parent().ok_or_else(|| {
        return observation_error("checkpoint path has no parent directory");
    })?;

    ensure_private_cache_directory(&SessionLayout::new(session).derived_cache())?;
    ensure_private_cache_directory(directory)?;

    let completion_bytes = read_completion_bytes(session)?;
    let state_bytes = serde_json::to_vec(&state.state).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<checkpoint-state>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let checkpoint = ReplayCheckpoint {
        version: REPLAY_CHECKPOINT_VERSION,
        state_format_version: STATE_FORMAT_VERSION,
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_point: point.clone(),
        completion_blake3: blake3::hash(&completion_bytes).to_hex().to_string(),
        final_event_hash: completion.final_event_hash.clone(),
        state_id: state.state_id,
        event_seq: state.event_seq,
        event_byte_offset: state.event_byte_offset,
        monotonic_ns: state.monotonic_ns,
        state_hash: state.state_hash.clone(),
        presented: state.presented,
        state: state.state.clone(),
        state_blake3: blake3::hash(&state_bytes).to_hex().to_string(),
    };
    let bytes = trust::encode(validated, checkpoint)?;
    let compressed = encode_all(bytes.as_slice(), 3).map_err(|error| {
        return observation_error(format!("could not compress replay checkpoint: {error}"));
    })?;

    if u64::try_from(compressed.len()).unwrap_or(u64::MAX) > MAX_REPLAY_CHECKPOINT_BYTES {
        tracing::warn!(path = %path.display(), "skipped oversized disposable replay checkpoint");

        return Ok(());
    }

    validated.check()?;

    write_secure(&path, &compressed)?;
    enforce_replay_cache_limits(session, Some(&path))?;

    return Ok(sync_directory(directory)?);
}

fn read_completion_bytes(session: &Path) -> Result<Vec<u8>> {
    let path = SessionLayout::new(session).completion();

    return fs_err::read(&path).map_err(|source| return Error::Filesystem { path, source });
}

fn checkpoint_path(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
) -> Result<PathBuf> {
    let encoded = serde_json::to_vec(point).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<state-point>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let key = blake3::hash(&encoded).to_hex().to_string();

    return Ok(SessionLayout::new(session).derived_cache().join(format!(
        "checkpoint-{}-{key}.json.zst",
        completion.final_event_hash
    )));
}

struct ReplayCacheEntry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// Inspect the bounded disposable replay cache without creating it.
///
/// # Errors
///
/// Returns an error when an existing cache directory or member is unsafe.
pub fn replay_cache_status(session: &Path) -> Result<ReplayCacheStatus> {
    let directory = SessionLayout::new(session).derived_cache();
    let entries = replay_cache_entries(&directory)?;

    return Ok(cache_status_from_entries(directory, &entries));
}

/// Delete every disposable replay checkpoint for one session.
///
/// Canonical evidence and manifested derived artifacts are never touched.
///
/// # Errors
///
/// Returns an error when the cache is unsafe or a checkpoint cannot be removed durably.
pub fn prune_replay_cache(session: &Path) -> Result<ReplayCachePruneReport> {
    let directory = SessionLayout::new(session).derived_cache();
    let entries = replay_cache_entries(&directory)?;
    let removed_entries = u64::try_from(entries.len()).unwrap_or(u64::MAX);
    let removed_bytes = entries.iter().fold(0_u64, |total, entry| {
        return total.saturating_add(entry.bytes);
    });

    for entry in entries {
        fs_err::remove_file(&entry.path).map_err(|source| {
            return Error::Filesystem {
                path: entry.path,
                source,
            };
        })?;
    }

    if directory.is_dir() {
        sync_directory(&directory)?;
    }

    let remaining = replay_cache_status(session)?;

    return Ok(ReplayCachePruneReport {
        directory,
        removed_entries,
        removed_bytes,
        remaining,
    });
}

fn enforce_replay_cache_limits(session: &Path, preserve: Option<&Path>) -> Result<()> {
    let directory = SessionLayout::new(session).derived_cache();
    let mut entries = replay_cache_entries(&directory)?;
    entries.sort_by(|left, right| {
        return left
            .modified
            .cmp(&right.modified)
            .then_with(|| return left.path.cmp(&right.path));
    });
    let mut count = u64::try_from(entries.len()).unwrap_or(u64::MAX);
    let mut bytes = entries.iter().fold(0_u64, |total, entry| {
        return total.saturating_add(entry.bytes);
    });

    for entry in entries {
        if count <= MAX_REPLAY_CACHE_ENTRIES && bytes <= MAX_REPLAY_CACHE_BYTES {
            break;
        }

        if preserve.is_some_and(|path| return path == entry.path) {
            continue;
        }

        fs_err::remove_file(&entry.path).map_err(|source| {
            return Error::Filesystem {
                path: entry.path.clone(),
                source,
            };
        })?;

        count = count.saturating_sub(1);
        bytes = bytes.saturating_sub(entry.bytes);
    }

    return Ok(sync_directory(&directory)?);
}

fn replay_cache_entries(directory: &Path) -> Result<Vec<ReplayCacheEntry>> {
    match fs_err::symlink_metadata(directory) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(observation_error(
                    "replay checkpoint directory must be a real owner-only directory",
                ));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            });
        }
    }

    let mut entries = Vec::new();
    for item in fs_err::read_dir(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })? {
        let item = item.map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;

        let name = item.file_name();
        if !name.as_encoded_bytes().starts_with(b"checkpoint-") {
            continue;
        }

        let path = item.path();
        let metadata = fs_err::symlink_metadata(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if !metadata.is_file() {
            return Err(observation_error(
                "replay checkpoint cache contains a non-regular member",
            ));
        }

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(observation_error(
                "replay checkpoint cache contains a non-owner-only member",
            ));
        }

        entries.push(ReplayCacheEntry {
            path,
            bytes: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }

    return Ok(entries);
}

fn cache_status_from_entries(
    directory: PathBuf,
    entries: &[ReplayCacheEntry],
) -> ReplayCacheStatus {
    return ReplayCacheStatus {
        directory,
        entries: u64::try_from(entries.len()).unwrap_or(u64::MAX),
        compressed_bytes: entries.iter().fold(0_u64, |total, entry| {
            return total.saturating_add(entry.bytes);
        }),
        maximum_entries: MAX_REPLAY_CACHE_ENTRIES,
        maximum_compressed_bytes: MAX_REPLAY_CACHE_BYTES,
    };
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
