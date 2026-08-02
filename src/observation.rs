//! Stable, truth-labeled terminal observation envelopes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zstd::stream::{decode_all, encode_all, read::Decoder};

use std::{
    fs::{File, OpenOptions},
    io::{BufReader, ErrorKind, Read as _},
    os::unix::ffi::OsStrExt as _,
    os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::SystemTime,
};

use nix::libc::O_NOFOLLOW;

use crate::{
    Error, Result,
    artifact::{create_private_directory, sync_directory, write_secure},
    control::{ControlCommand, ControlData, request},
    event::{ByteString, Event},
    event_log::{CanonicalBoundary, EventLog},
    ghostty_vt::{Format, GhosttyVt, ModeState, SemanticState},
    replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION, StateInspection, StateSelector, inspect},
    session::{SessionCompletion, read_completion, read_manifest, validate_completed_log},
};

/// Source category for one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSourceKind {
    /// Responsive canonical recording actor.
    LiveSession,
    /// Validated completed canonical recording.
    CompletedSession,
    /// Explicitly validated but incomplete canonical event prefix.
    InterruptedSession,
    /// Unchained ANSI/VT byte stream.
    AnsiStream,
    /// Completed canonical evidence imported from an ANSI/VT byte stream.
    ImportedAnsi,
    /// Canonical recording allocated for a one-off command.
    OneOffCommand,
}

/// How settled the observed state is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureQuality {
    /// Immediate state from a responsive live actor.
    Live,
    /// Live state proven unchanged for a requested interval.
    Stable,
    /// State from validated completed evidence.
    Completed,
    /// Explicitly incomplete validated prefix.
    Incomplete,
}

/// Rendering truth represented by an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TruthLayer {
    /// Pinned semantic-engine state.
    Semantic,
    /// Pixels captured from the native terminal renderer.
    Native,
}

/// Integrity relationship between an observation and canonical evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationIntegrity {
    /// State derives from a validated canonical hash chain or live actor boundary.
    CanonicalChained,
    /// State derives from an unchained byte stream.
    Unchained,
}

/// Service path used to resolve an observation without changing its meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationResolution {
    /// Actor-consistent live snapshot fast path.
    ActorSnapshot,
    /// Independently validated canonical event replay.
    CanonicalReplay,
    /// Content-addressed full-state checkpoint validated against exact source bytes.
    ValidatedCheckpoint,
    /// Pinned semantic interpretation of an unchained ANSI stream.
    AnsiInterpretation,
}

/// Disposable full-state checkpoint bound to exact canonical source files.
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
    /// Complete reconstructed state and provenance.
    pub inspection: StateInspection,
    /// BLAKE3 of canonical JSON for the complete cached semantic state.
    pub inspection_state_blake3: String,
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

/// Shared state selection vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "selector", rename_all = "snake_case", deny_unknown_fields)]
pub enum StatePoint {
    /// Latest presentation-eligible state.
    LatestPresented,
    /// Exact semantic state identifier.
    StateId {
        /// Exact semantic state identifier.
        state_id: u64,
    },
    /// Exact content-addressed semantic state hash.
    StateHash {
        /// BLAKE3 hash of canonical semantic state JSON.
        state_hash: String,
    },
    /// Latest state at or before an event sequence.
    EventSeq {
        /// Canonical event sequence.
        event_seq: u64,
    },
    /// Latest state at or before a session-monotonic timestamp.
    MonotonicNs {
        /// Session-monotonic nanoseconds.
        monotonic_ns: u64,
    },
    /// State at a uniquely named marker.
    Marker {
        /// Unique marker label.
        label: String,
    },
}

/// Compact stable frame shared by CLI and clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationFrame {
    /// Plain text for the complete viewport.
    pub viewport_text: String,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Cursor column when it lies in the viewport.
    pub cursor_column: Option<u16>,
    /// Cursor row when it lies in the viewport.
    pub cursor_row: Option<u16>,
    /// Whether the cursor is visible.
    pub cursor_visible: bool,
    /// Stable terminal-mode facts.
    pub modes: ModeState,
}

/// Compact versioned frame JSON optimized for test clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameEnvelope {
    /// Stable frame format version.
    pub version: u32,
    /// Semantic state hash linked to the full observation.
    pub state_hash: String,
    /// Whether this state was eligible for terminal presentation.
    pub presented: bool,
    /// Settlement quality of the source observation.
    pub capture_quality: CaptureQuality,
    /// Compact frame payload.
    pub frame: ObservationFrame,
    /// BLAKE3 of canonical JSON for `frame`.
    pub frame_hash: String,
}

/// Truth-labeled observation of one terminal state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Source category.
    pub source_kind: ObservationSourceKind,
    /// Canonical session ID when evidence exists.
    pub session_id: Option<Uuid>,
    /// Exact canonical session path bytes when evidence exists.
    pub canonical_session: Option<ByteString>,
    /// Whether canonical execution evidence was durably completed.
    pub source_completed: bool,
    /// Requested state point.
    pub state_point: StatePoint,
    /// Semantic state ID when replay assigned one.
    pub state_id: Option<u64>,
    /// BLAKE3 of the complete semantic state.
    pub state_hash: String,
    /// Responsible canonical event sequence when known.
    pub event_seq: Option<u64>,
    /// Session-monotonic timestamp when known.
    pub monotonic_ns: Option<u64>,
    /// Whether this state was presentation eligible.
    pub presented: bool,
    /// Source boundary observed or validated.
    pub boundary: CanonicalBoundary,
    /// Settlement quality.
    pub capture_quality: CaptureQuality,
    /// Rendering truth layer.
    pub truth_layer: TruthLayer,
    /// Integrity relationship to canonical execution evidence.
    pub integrity: ObservationIntegrity,
    /// Service path used to resolve this state.
    pub resolution: ObservationResolution,
    /// Compact stable frame.
    pub frame: ObservationFrame,
    /// BLAKE3 of canonical JSON for `frame`.
    pub payload_hash: String,
}

/// Observe a responsive live session or validated completed session.
///
/// # Errors
///
/// Returns an error when canonical identity/validation fails, a live selector
/// is not supported, actor observation fails, or replay cannot resolve the point.
pub fn observe_session(session: &Path, state_point: StatePoint) -> Result<Observation> {
    return observe_session_with_options(session, state_point, false);
}

/// Observe a session with explicit permission to select hidden semantic states.
///
/// # Errors
///
/// Returns the same errors as [`observe_session`] and rejects a hidden selected
/// state unless `include_hidden` is true.
pub fn observe_session_with_options(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    let manifest = read_manifest(&canonical)?;
    if canonical.join("completion.json").is_file() {
        return observe_completed(&canonical, manifest.session_id, state_point, include_hidden);
    }
    if state_point != StatePoint::LatestPresented {
        return Err(observation_error(
            "live sessions currently support only latest-presented selection",
        ));
    }
    let snapshot = request(&canonical, ControlCommand::Snapshot)?;
    let ControlData::Snapshot {
        semantic_state_zstd: _,
        viewport_text,
        state_hash,
        presented,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
        observed_monotonic_ns,
        boundaries,
    } = snapshot
    else {
        return Err(observation_error(
            "actor returned an unexpected observation",
        ));
    };
    let frame = ObservationFrame {
        viewport_text,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        modes,
    };
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::LiveSession,
        session_id: Some(manifest.session_id),
        canonical_session: Some(ByteString::from(canonical.as_os_str().as_bytes())),
        source_completed: false,
        state_point,
        state_id: None,
        state_hash,
        event_seq: boundaries.observed.next_event_seq.checked_sub(1),
        monotonic_ns: Some(observed_monotonic_ns),
        presented,
        boundary: boundaries.observed,
        capture_quality: CaptureQuality::Live,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution: ObservationResolution::ActorSnapshot,
        frame,
        payload_hash,
    });
}

/// Observe an explicitly requested validated incomplete canonical prefix.
///
/// # Errors
///
/// Returns an error when completion exists, the event prefix is invalid, the
/// selector cannot resolve, or a hidden state was not explicitly permitted.
pub fn observe_validated_prefix(
    session: &Path,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if canonical.join("completion.json").exists() {
        return Err(observation_error(
            "validated-prefix observation requires incomplete evidence",
        ));
    }
    let manifest = read_manifest(&canonical)?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let inspection = inspect(&log, state_selector(&log, &point)?)?;
    if !inspection.record.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    let output_bytes = log.events.iter().fold(0_u64, |total, record| {
        return total.saturating_add(match &record.event {
            Event::Output { data } => u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX),
            _ => 0,
        });
    });
    let event_hash = match log.events.last() {
        Some(record) => record.hash.clone(),
        None => {
            let bytes = serde_json::to_vec(&log.header).map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<event-header>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            blake3::hash(&bytes).to_hex().to_string()
        }
    };
    return Ok(Observation {
        source_kind: ObservationSourceKind::InterruptedSession,
        session_id: Some(manifest.session_id),
        canonical_session: Some(ByteString::from(canonical.as_os_str().as_bytes())),
        source_completed: false,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash,
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
        boundary: CanonicalBoundary {
            next_event_seq: u64::try_from(log.events.len()).unwrap_or(u64::MAX),
            event_hash,
            output_bytes,
        },
        capture_quality: CaptureQuality::Incomplete,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution: ObservationResolution::CanonicalReplay,
        frame,
        payload_hash,
    });
}

fn observe_completed(
    session: &Path,
    session_id: Uuid,
    point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let completion = read_completion(session)?;
    if let Some(inspection) = load_checkpoint(session, &point, &completion)? {
        if !inspection.record.presented && !include_hidden {
            return Err(observation_error(
                "selected semantic state is hidden; pass explicit hidden-state permission",
            ));
        }
        return completed_observation_from_inspection(
            session,
            session_id,
            point,
            &completion,
            &inspection,
            ObservationResolution::ValidatedCheckpoint,
        );
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    let completion = validate_completed_log(session, &log)?;
    let selector = state_selector(&log, &point)?;
    let inspection = inspect(&log, selector)?;
    if !inspection.record.presented && !include_hidden {
        return Err(observation_error(
            "selected semantic state is hidden; pass explicit hidden-state permission",
        ));
    }
    if let Err(error) = publish_checkpoint(session, &point, &completion, &inspection) {
        tracing::warn!(%error, "could not publish disposable replay checkpoint");
    }
    return completed_observation_from_inspection(
        session,
        session_id,
        point,
        &completion,
        &inspection,
        ObservationResolution::CanonicalReplay,
    );
}

pub(crate) fn completed_observation_from_inspection(
    session: &Path,
    session_id: Uuid,
    point: StatePoint,
    completion: &SessionCompletion,
    inspection: &StateInspection,
    resolution: ObservationResolution,
) -> Result<Observation> {
    let frame = frame_from_state(&inspection.state);
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::CompletedSession,
        session_id: Some(session_id),
        canonical_session: Some(ByteString::from(session.as_os_str().as_bytes())),
        source_completed: true,
        state_point: point,
        state_id: Some(inspection.record.state_id),
        state_hash: inspection.record.state_hash.clone(),
        event_seq: inspection.record.event_seq,
        monotonic_ns: Some(inspection.record.monotonic_ns),
        presented: inspection.record.presented,
        boundary: CanonicalBoundary {
            next_event_seq: completion.event_count,
            event_hash: completion.final_event_hash.clone(),
            output_bytes: completion.output_bytes,
        },
        capture_quality: CaptureQuality::Completed,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::CanonicalChained,
        resolution,
        frame,
        payload_hash,
    });
}

pub(crate) fn state_selector(log: &EventLog, point: &StatePoint) -> Result<StateSelector> {
    let selector = match point {
        StatePoint::LatestPresented => StateSelector::LatestPresented,
        StatePoint::StateId { state_id } => StateSelector::StateId(*state_id),
        StatePoint::StateHash { state_hash } => StateSelector::StateHash(state_hash.clone()),
        StatePoint::EventSeq { event_seq } => StateSelector::EventSeq(*event_seq),
        StatePoint::MonotonicNs { monotonic_ns } => StateSelector::MonotonicNs(*monotonic_ns),
        StatePoint::Marker { label } => {
            let mut matches = log.events.iter().filter(|record| {
                return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
            });
            let event_seq = matches
                .next()
                .map(|record| return record.seq)
                .ok_or_else(|| return observation_error("marker does not exist"))?;
            if matches.next().is_some() {
                return Err(observation_error("marker label is ambiguous"));
            }
            StateSelector::EventSeq(event_seq)
        }
    };
    return Ok(selector);
}

fn load_checkpoint(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
) -> Result<Option<StateInspection>> {
    let path = checkpoint_path(session, point, completion)?;
    match fs_err::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Filesystem { path, source });
        }
    }
    let validate = (|| -> Result<StateInspection> {
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
        let state_bytes = serde_json::to_vec(&checkpoint.inspection.state).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<checkpoint-state>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let state_hash = blake3::hash(&state_bytes).to_hex().to_string();
        if checkpoint.version != 1
            || checkpoint.state_format_version != STATE_FORMAT_VERSION
            || checkpoint.ghostty_revision != GHOSTTY_REVISION
            || checkpoint.state_point != *point
            || checkpoint.final_event_hash != completion.final_event_hash
            || checkpoint.completion_blake3 != blake3::hash(&completion_bytes).to_hex().to_string()
            || checkpoint.event_log_byte_count != event_log_byte_count
            || checkpoint.event_log_blake3 != event_log_blake3
            || checkpoint.inspection_state_blake3 != state_hash
        {
            return Err(observation_error(
                "replay checkpoint does not match exact canonical source bytes",
            ));
        }
        return Ok(checkpoint.inspection);
    })();
    return match validate {
        Ok(inspection) => Ok(Some(inspection)),
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

fn publish_checkpoint(
    session: &Path,
    point: &StatePoint,
    completion: &SessionCompletion,
    inspection: &StateInspection,
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
    let state_bytes = serde_json::to_vec(&inspection.state).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<checkpoint-state>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let checkpoint = ReplayCheckpoint {
        version: 1,
        state_format_version: STATE_FORMAT_VERSION,
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_point: point.clone(),
        event_log_byte_count,
        event_log_blake3,
        completion_blake3: blake3::hash(&completion_bytes).to_hex().to_string(),
        final_event_hash: completion.final_event_hash.clone(),
        inspection: inspection.clone(),
        inspection_state_blake3: blake3::hash(&state_bytes).to_hex().to_string(),
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

/// Render an unchained ANSI/VT stream into a truth-labeled observation.
///
/// # Errors
///
/// Returns an error when geometry or semantic-state capture is invalid.
pub fn observe_ansi_stream(bytes: &[u8], cols: u16, rows: u16) -> Result<Observation> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    let state = terminal.snapshot()?;
    let state_bytes = serde_json::to_vec(&state).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<ansi-state>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let state_hash = blake3::hash(&state_bytes).to_hex().to_string();
    let frame = frame_from_state(&state);
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::AnsiStream,
        session_id: None,
        canonical_session: None,
        source_completed: false,
        state_point: StatePoint::LatestPresented,
        state_id: None,
        state_hash,
        event_seq: None,
        monotonic_ns: None,
        presented: true,
        boundary: CanonicalBoundary {
            next_event_seq: 0,
            event_hash: blake3::hash(bytes).to_hex().to_string(),
            output_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        },
        capture_quality: CaptureQuality::Incomplete,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::Unchained,
        resolution: ObservationResolution::AnsiInterpretation,
        frame,
        payload_hash,
    });
}

/// Resolve the complete semantic state behind a session observation.
///
/// # Errors
///
/// Returns an error when the source is invalid, the selector is unsupported for
/// a live actor, or hidden-state permission is absent.
pub fn semantic_state_for_session(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<SemanticState> {
    let (state, _) = semantic_state_with_hash_for_session(session, point, include_hidden)?;
    return Ok(state);
}

/// Resolve a complete semantic state with the exact hash assigned by its source.
///
/// The paired hash preserves the source's hash contract: completed evidence uses
/// its canonical replay hash, while a live actor returns its snapshot hash.
///
/// # Errors
///
/// Returns an error when the source is invalid, the selector is unsupported for
/// a live actor, or hidden-state permission is absent.
pub fn semantic_state_with_hash_for_session(
    session: &Path,
    point: &StatePoint,
    include_hidden: bool,
) -> Result<(SemanticState, String)> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if canonical.join("completion.json").is_file() {
        let log = EventLog::read(canonical.join("events.jsonl"))?;
        validate_completed_log(&canonical, &log)?;
        let selector = state_selector(&log, point)?;
        let inspection = inspect(&log, selector)?;
        if !inspection.record.presented && !include_hidden {
            return Err(observation_error(
                "selected semantic state is hidden; pass explicit hidden-state permission",
            ));
        }
        return Ok((inspection.state, inspection.record.state_hash));
    }
    if point != &StatePoint::LatestPresented || include_hidden {
        return Err(observation_error(
            "live semantic images support only latest-presented selection",
        ));
    }
    let ControlData::Snapshot {
        semantic_state_zstd,
        state_hash,
        ..
    } = request(&canonical, ControlCommand::Snapshot)?
    else {
        return Err(observation_error(
            "actor returned an unexpected semantic snapshot",
        ));
    };
    let decoded = decode_all(semantic_state_zstd.as_bytes()).map_err(|error| {
        return observation_error(format!("decompress live semantic snapshot: {error}"));
    })?;
    if decoded.len() > 32 * 1024 * 1024 {
        return Err(observation_error(
            "live semantic snapshot exceeds the 32 MiB decoded limit",
        ));
    }
    let state = serde_json::from_slice(&decoded).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<live-semantic-snapshot>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok((state, state_hash));
}

/// Interpret an ANSI/VT stream into one complete semantic state.
///
/// # Errors
///
/// Returns an error when terminal geometry or semantic capture is invalid.
pub fn semantic_state_from_ansi(bytes: &[u8], cols: u16, rows: u16) -> Result<SemanticState> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    return terminal.snapshot();
}

impl Observation {
    /// Return the compact stable frame contract linked to this observation.
    #[must_use]
    pub fn frame_envelope(&self) -> FrameEnvelope {
        return FrameEnvelope {
            version: 1,
            state_hash: self.state_hash.clone(),
            presented: self.presented,
            capture_quality: self.capture_quality,
            frame: self.frame.clone(),
            frame_hash: self.payload_hash.clone(),
        };
    }
}

/// Format an unchained ANSI stream through the pinned Ghostty formatter.
///
/// # Errors
///
/// Returns an error when geometry or formatter output is invalid.
pub fn format_ansi_stream(bytes: &[u8], cols: u16, rows: u16, format: Format) -> Result<Vec<u8>> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    return terminal.format(format);
}

/// Observe a completed canonical session created for a one-off command.
///
/// # Errors
///
/// Returns an error when the session is incomplete, invalid, or the requested
/// state cannot be resolved.
pub fn observe_one_off_session(
    session: &Path,
    state_point: StatePoint,
    include_hidden: bool,
) -> Result<Observation> {
    let mut observation = observe_session_with_options(session, state_point, include_hidden)?;
    if !observation.source_completed {
        return Err(observation_error(
            "one-off command did not produce completed canonical evidence",
        ));
    }
    observation.source_kind = ObservationSourceKind::OneOffCommand;
    return Ok(observation);
}

pub(crate) fn frame_from_state(state: &SemanticState) -> ObservationFrame {
    return ObservationFrame {
        viewport_text: viewport_text(state),
        cols: state.cols,
        rows: state.rows,
        cursor_column: state.cursor.column,
        cursor_row: state.cursor.row,
        cursor_visible: state.cursor.visible,
        modes: state.modes.clone(),
    };
}

fn viewport_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        let start = output.len();
        for cell in &row.cells {
            if cell.grapheme.is_empty() {
                output.push(' ');
            } else {
                output.push_str(&cell.grapheme);
            }
        }
        while output.len() > start && output.ends_with(' ') {
            output.pop();
        }
        if index + 1 < state.grid.len() {
            output.push('\n');
        }
    }
    return output;
}

pub(crate) fn frame_hash(frame: &ObservationFrame) -> Result<String> {
    let bytes = serde_json::to_vec(frame).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<observation-frame>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(blake3::hash(&bytes).to_hex().to_string());
}

fn observation_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "observe terminal state",
        reason: reason.into(),
    };
}
