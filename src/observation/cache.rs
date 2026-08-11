use super::*;

/// Disposable full-state checkpoint bound to exact canonical source bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCheckpoint {
    /// Checkpoint format version.
    pub version: u32,
    /// Pinned semantic state format.
    pub state_format_version: u32,
    /// Pinned semantic engine revision.
    pub ghostty_revision: String,
    /// Exact requested state point.
    pub state_point: StatePoint,
    /// Exact canonical event-log byte count.
    pub event_log_byte_count: u64,
    /// BLAKE3 of the complete canonical event-log file.
    pub event_log_blake3: String,
    /// BLAKE3 of exact completion JSON bytes.
    pub completion_blake3: String,
    /// Completion-bound final canonical event hash.
    pub final_event_hash: String,
    /// Dense canonical state identifier when byte-granular replay established it.
    pub state_id: Option<u64>,
    /// Canonical event responsible for the state, when known.
    pub event_seq: Option<u64>,
    /// Byte offset inside the source output event, when known.
    pub event_byte_offset: Option<u64>,
    /// Monotonic source timestamp, when known.
    pub monotonic_ns: Option<u64>,
    /// BLAKE3 semantic-state hash.
    pub state_hash: String,
    /// Whether the state was eligible for terminal presentation.
    pub presented: bool,
    /// Complete semantic state.
    pub state: SemanticState,
    /// BLAKE3 of canonical JSON for the complete cached semantic state.
    pub state_blake3: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CachedSemanticState {
    pub state_id: Option<u64>,
    pub event_seq: Option<u64>,
    pub event_byte_offset: Option<u64>,
    pub monotonic_ns: Option<u64>,
    pub state_hash: String,
    pub presented: bool,
    pub state: SemanticState,
}

/// Bounded disposable replay-cache inventory for one canonical session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCacheStatus {
    /// Cache directory inside the canonical session.
    pub directory: PathBuf,
    /// Number of checkpoint files currently retained.
    pub entries: u64,
    /// Total compressed bytes retained.
    pub compressed_bytes: u64,
    /// Automatic per-session checkpoint limit.
    pub maximum_entries: u64,
    /// Automatic per-session compressed-byte limit.
    pub maximum_compressed_bytes: u64,
}

/// Result of explicitly deleting disposable replay checkpoints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayCachePruneReport {
    /// Cache directory considered.
    pub directory: PathBuf,
    /// Files removed.
    pub removed_entries: u64,
    /// Compressed bytes removed.
    pub removed_bytes: u64,
    /// Inventory remaining after pruning.
    pub remaining: ReplayCacheStatus,
}

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
        let file_type = metadata.file_type();
        if file_type.is_dir()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
            || metadata.permissions().mode() & 0o077 != 0
        {
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
        let checkpoint: ReplayCheckpoint = serde_json::from_slice(&bytes).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let completion_bytes = fs_err::read(session.join("completion.json")).map_err(|source| {
            return Error::Filesystem {
                path: session.join("completion.json"),
                source,
            };
        })?;
        let event_path = session.join("events.jsonl");
        let (event_log_byte_count, event_log_blake3) = hash_file(&event_path)?;
        let state_bytes = serde_json::to_vec(&checkpoint.state).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<checkpoint-state>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let state_blake3 = blake3::hash(&state_bytes).to_hex().to_string();
        let semantic_hash = semantic_state_hash(&checkpoint.state)?;
        if checkpoint.version != 2
            || checkpoint.state_format_version != STATE_FORMAT_VERSION
            || checkpoint.ghostty_revision != GHOSTTY_REVISION
            || checkpoint.state_point != *point
            || checkpoint.final_event_hash != completion.final_event_hash
            || checkpoint.completion_blake3 != blake3::hash(&completion_bytes).to_hex().to_string()
            || checkpoint.event_log_byte_count != event_log_byte_count
            || checkpoint.event_log_blake3 != event_log_blake3
            || checkpoint.state_blake3 != state_blake3
            || checkpoint.state_hash != semantic_hash
        {
            return Err(observation_error(
                "replay checkpoint does not match exact canonical source bytes",
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
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
    inspection: &StateInspection,
) -> Result<()> {
    let state = CachedSemanticState {
        state_id: Some(inspection.record.state_id),
        event_seq: inspection.record.event_seq,
        event_byte_offset: inspection.record.event_byte_offset,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        state_hash: inspection.record.state_hash.clone(),
        presented: inspection.record.presented,
        state: inspection.state.clone(),
    };
    return publish_cached_checkpoint(session, point, completion, &state);
}

pub(crate) fn publish_endpoint_checkpoint(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
    inspection: &EndpointInspection,
) -> Result<()> {
    let state = CachedSemanticState {
        state_id: None,
        event_seq: inspection.event_seq,
        event_byte_offset: inspection.event_byte_offset,
        monotonic_ns: inspection.monotonic_ns,
        state_hash: inspection.state_hash.clone(),
        presented: inspection.presented,
        state: inspection.state.clone(),
    };
    return publish_cached_checkpoint(session, point, completion, &state);
}

fn publish_cached_checkpoint(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
    state: &CachedSemanticState,
) -> Result<()> {
    let path = checkpoint_path(session, point, completion)?;
    if path.exists() {
        return Ok(());
    }
    let directory = path.parent().ok_or_else(|| {
        return observation_error("checkpoint path has no parent directory");
    })?;
    ensure_private_cache_directory(&session.join("derived-cache"))?;
    ensure_private_cache_directory(directory)?;
    let completion_bytes = fs_err::read(session.join("completion.json")).map_err(|source| {
        return Error::Filesystem {
            path: session.join("completion.json"),
            source,
        };
    })?;
    let (event_log_byte_count, event_log_blake3) = hash_file(&session.join("events.jsonl"))?;
    let state_bytes = serde_json::to_vec(&state.state).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<checkpoint-state>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let checkpoint = ReplayCheckpoint {
        version: 2,
        state_format_version: STATE_FORMAT_VERSION,
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_point: point.clone(),
        event_log_byte_count,
        event_log_blake3,
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
    let bytes = serde_json::to_vec(&checkpoint).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let compressed = encode_all(bytes.as_slice(), 3).map_err(|error| {
        return observation_error(format!("could not compress replay checkpoint: {error}"));
    })?;
    if u64::try_from(compressed.len()).unwrap_or(u64::MAX) > MAX_REPLAY_CHECKPOINT_BYTES {
        tracing::warn!(path = %path.display(), "skipped oversized disposable replay checkpoint");
        return Ok(());
    }
    write_secure(&path, &compressed)?;
    enforce_replay_cache_limits(session, Some(&path))?;
    return sync_directory(directory);
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
    return Ok(session.join("derived-cache").join(format!(
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
    let directory = session.join("derived-cache");
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
    let directory = session.join("derived-cache");
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
    let directory = session.join("derived-cache");
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
    return sync_directory(&directory);
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
        let file_type = metadata.file_type();
        if file_type.is_dir()
            || file_type.is_symlink()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
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

fn ensure_private_cache_directory(path: &Path) -> Result<()> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(observation_error(
                    "replay checkpoint directory must be a real directory",
                ));
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(observation_error(
                    "replay checkpoint directory must be owner-only",
                ));
            }
            return Ok(());
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return create_private_directory(path);
        }
        Err(source) => {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source,
            });
        }
    }
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut byte_count = 0_u64;
    let mut buffer = vec![0_u8; 128 * 1024].into_boxed_slice();
    loop {
        let read = reader.read(&mut buffer).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        if read == 0 {
            break;
        }
        let bytes = buffer.get(..read).ok_or_else(|| {
            return observation_error("checkpoint hash buffer bounds are invalid");
        })?;
        hasher.update(bytes);
        byte_count = byte_count.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    return Ok((byte_count, hasher.finalize().to_hex().to_string()));
}
