use super::{
    invalid_replay, json_error, replay_for_index, trust::SourceIdentity, ReplaySeed, ReplayStop,
    ReplayVisit, POSITION_STRIDE,
};
use crate::completion;
use crate::{timeline::resolve_source_range, Error, Result};
use serde::Serialize;
use std::{
    cell::RefCell,
    env,
    fmt::Write as _,
    fs::File,
    io::{BufRead, BufReader, BufWriter, ErrorKind, Read as _, Seek as _, SeekFrom, Write},
    num::TryFromIntError,
    path::{Path, PathBuf},
};
use termtrace_evidence::{
    artifact::{
        append_artifact_root, collect_artifact_members, commit_directory, create_private_directory,
        finish_writer, open_verified_artifact, secure_file, secure_writer, sync_directory,
        verify_artifact_members, write_secure,
    },
    event_log::{EventLog, EventLogSummary},
    session::{read_completion, validate_completion_summary_candidate},
};
use termtrace_model::{
    artifact::metadata::ArtifactSourceRange,
    execution::event::{Event, EventLogPosition},
    execution::layout::{self, SessionLayout},
    execution::session::SessionCompletion,
    terminal::replay::{
        IndexOptions, IndexOutcome, ReplayArtifacts, ReplayManifest, SemanticReplayCheckpoint,
        SourceRange, StateIndexEntry, StateRecord, StateSelection, GHOSTTY_REVISION,
        STATE_FORMAT_VERSION, STATE_INDEX,
    },
};
use termtrace_terminal::GhosttyVt;
use uuid::Uuid;
use zstd::{
    bulk::Compressor,
    stream::{decode_all, write::Encoder as ZstdEncoder},
};

mod control;
mod rank;
mod seekable;
mod source;
#[cfg(test)]
mod tests;

use control::write_control_index;
use rank::TimelineRanks;
pub(crate) use seekable::{open_seekable, seekable_len, SeekableReader};
use source::IndexSource;

// Seeks restore a checkpoint and replay from it, so the positions between checkpoints grow with
// the size of their frames: frames stay a small fraction of the index on long recordings.
const CHECKPOINT_SPACING: CheckpointSpacing = CheckpointSpacing {
    minimum_units: 4 * POSITION_STRIDE,
    units_per_frame_byte: 16,
};

#[cfg(test)]
pub(super) const TEST_CHECKPOINT_SPACING: CheckpointSpacing = CheckpointSpacing {
    minimum_units: 32,
    units_per_frame_byte: 0,
};

const INDEX_MANIFEST: &str = "manifest.json";
const CHECKPOINTS: &str = "checkpoints.jsonl.zst";
const CHECKPOINT_FRAMES: &str = "checkpoints.bin";
const CHECKPOINT_COMPRESSION_LEVEL: i32 = 3;
pub(crate) const CONTROL_INDEX: &str = "control-index.jsonl.zst";
pub(crate) const CONTROL_SOURCE_INDEX: &str = "control-source-index.jsonl.zst";

/// Generate replay artifacts into an atomically committed `derived` directory.
///
/// # Errors
///
/// Returns an error for invalid source logs, existing artifacts, replay
/// failures, serialization failures, or non-durable filesystem operations.
pub fn index_session(session_directory: impl AsRef<Path>) -> Result<IndexOutcome> {
    return index_session_with_options(session_directory, IndexOptions::default());
}

/// Generate replay artifacts for a validated half-open source-time interval.
///
/// # Errors
///
/// Returns an error for invalid bounds or any full-index failure.
pub fn index_session_range(
    session_directory: impl AsRef<Path>,
    start_monotonic_ns: Option<u64>,
    end_monotonic_ns: Option<u64>,
) -> Result<IndexOutcome> {
    return index_session_with_options(
        session_directory,
        IndexOptions {
            start_monotonic_ns,
            end_monotonic_ns,
            ..IndexOptions::default()
        },
    );
}

/// Generate video-optimized replay artifacts sampled at 1/FPS increments.
///
/// # Errors
///
/// Returns an error for invalid source logs, existing artifacts, replay
/// failures, serialization failures, or non-durable filesystem operations.
pub fn index_session_for_video(
    session_directory: impl AsRef<Path>,
    fps: u32,
) -> Result<IndexOutcome> {
    return index_session_with_options(session_directory, IndexOptions::for_video(fps));
}

/// Generate replay artifacts sampled at an explicit monotonic nanosecond stride.
///
/// # Errors
///
/// Returns an error for invalid source logs, existing artifacts, replay
/// failures, serialization failures, or non-durable filesystem operations.
pub fn index_session_with_stride(
    session_directory: impl AsRef<Path>,
    stride_ns: u64,
) -> Result<IndexOutcome> {
    return index_session_with_options(
        session_directory,
        IndexOptions::default().with_sampling_stride(stride_ns),
    );
}

/// Generate replay artifacts under explicit completion and retention policy.
///
/// # Errors
///
/// Returns an error for incomplete sources unless `live_snapshot` is enabled,
/// invalid bounds, existing artifacts, replay failures, or artifact I/O.
pub fn index_session_with_options(
    session_directory: impl AsRef<Path>,
    options: IndexOptions,
) -> Result<IndexOutcome> {
    let session_directory = session_directory.as_ref();
    let session_layout = SessionLayout::new(session_directory);
    let completion_exists = completion::published(session_directory)?.is_some();

    if !completion_exists && !options.live_snapshot {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }

    let event_path = session_layout.event_log();
    let identity = completion_exists
        .then(|| return SourceIdentity::capture(session_directory))
        .transpose()?;

    let summary = if completion_exists {
        EventLogSummary::read(&event_path)?
    } else {
        EventLogSummary::snapshot(&event_path)?
    };

    let source_complete = if completion_exists {
        validate_completion_summary_candidate(
            session_directory,
            &summary,
            &read_completion(session_directory)?,
        )?;

        true
    } else {
        false
    };

    let final_monotonic_ns = summary
        .terminal
        .as_ref()
        .map_or(0, |record| return record.monotonic_ns);

    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_monotonic_ns,
    )?;

    let mut directory_name = derived_directory_name_for(source_range, options.state_selection);

    if !source_complete {
        let source_hash = summary.terminal.as_ref().map_or_else(
            || return summary.header.session_id.simple().to_string(),
            |record| {
                return record.hash.clone();
            },
        );

        let short_hash = source_hash.get(..12).unwrap_or(source_hash.as_str());
        write!(directory_name, "-snapshot-{short_hash}")
            .map_err(|_| return invalid_replay("format live snapshot directory name"))?;
    }

    let destination = session_directory.join(directory_name);
    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }

    let staging = session_directory.join(format!(".derived.{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;

    let sampling_stride_ns = options.sampling_stride_ns.or_else(|| {
        return env::var("TERMTRACE_INDEX_SAMPLING_STRIDE_NS")
            .ok()
            .and_then(|v| return v.parse::<u64>().ok())
            .filter(|&v| return v > 0)
            .or_else(|| {
                return env::var("TERMTRACE_INDEX_FPS")
                    .ok()
                    .and_then(|v| return v.parse::<u32>().ok())
                    .filter(|&fps| return fps > 0)
                    .map(|fps| return 1_000_000_000_u64 / u64::from(fps));
            });
    });

    let result = write_source_index(
        &staging,
        &IndexSource::File(&event_path, &summary),
        source_range,
        source_complete,
        options.state_selection,
        sampling_stride_ns,
        CHECKPOINT_SPACING,
    )
    .and_then(|(artifacts, retained_state_count)| {
        if let Some(identity) = identity.as_ref() {
            identity.check(session_directory)?;
        }

        commit_directory(session_directory, &staging, &destination, false)?;
        let manifest_path = destination.join(INDEX_MANIFEST);
        let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
            return Error::Filesystem {
                path: manifest_path,
                source,
            };
        })?;

        if source_complete {
            append_artifact_root(
                session_directory,
                &destination,
                "semantic_index",
                &manifest_bytes,
                &summary.header.session_manifest_hash,
                summary
                    .terminal
                    .as_ref()
                    .map(|record| return record.hash.as_str()),
                source_range.map(|range| {
                    return ArtifactSourceRange {
                        start_monotonic_ns: range.start_monotonic_ns,
                        end_monotonic_ns: range.end_monotonic_ns,
                    };
                }),
            )?;
        }

        return Ok(IndexOutcome {
            directory: destination.clone(),
            state_count: retained_state_count,
            final_state_id: artifacts.final_state_id,
            source_complete,
            state_selection: options.state_selection,
            final_state_hash: artifacts.final_state_hash,
        });
    });
    if let Err(primary) = result {
        if staging.exists()
            && let Err(source) = fs_err::remove_dir_all(&staging)
        {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(Error::Filesystem {
                    path: staging,
                    source,
                }),
            });
        }
        return Err(primary);
    }
    return result;
}

/// Resolve the deterministic derived artifact directory for a source range.
pub fn derived_directory_name(source_range: Option<SourceRange>) -> String {
    return derived_directory_name_for(source_range, StateSelection::All);
}

/// Resolve a deterministic derived directory for a range and retention mode.
pub fn derived_directory_name_for(
    source_range: Option<SourceRange>,
    state_selection: StateSelection,
) -> String {
    let base = match state_selection {
        StateSelection::All => layout::DERIVED.to_owned(),
        StateSelection::Presented => format!("{}-presented", layout::DERIVED),
    };

    return source_range.map_or_else(
        || return base.clone(),
        |range| return format!("{base}-{}", range.artifact_suffix()),
    );
}

/// A session's default semantic index, reachable only through the derived-artifact trust rule.
///
/// The index is a read model: it is opened only when the session's artifact catalog binds its
/// exact manifest to the replayed log and every member matches, and an index built by another
/// engine revision is ignored so replay falls back to canonical events.
pub(crate) struct DerivedIndex {
    directory: PathBuf,
    final_state_id: u64,
    latest_presented_state_id: Option<u64>,
    sampled: bool,
}

impl DerivedIndex {
    /// Open the default index of `log`'s session, or `None` when no trusted index exists.
    ///
    /// # Errors
    ///
    /// Returns an error when a cataloged index fails its catalog binding or member verification.
    pub(crate) fn open(log: &EventLog) -> Result<Option<Self>> {
        let Some(session) = log.source_path.as_deref().and_then(Path::parent) else {
            return Ok(None);
        };

        return Self::open_source(
            session,
            &log.header.session_manifest_hash,
            log.events.last().map(|record| return record.hash.as_str()),
        );
    }

    /// Open the default index of a completed session without loading its event log.
    ///
    /// # Errors
    ///
    /// Returns an error when a cataloged index fails its catalog binding or member verification.
    pub(crate) fn open_completed(
        session: &Path,
        completion: &SessionCompletion,
    ) -> Result<Option<Self>> {
        return Self::open_source(
            session,
            &completion.session_manifest_hash,
            Some(completion.final_event_hash.as_str()),
        );
    }

    // Member classification and the manifest schema follow the format version, so an index
    // written by another version is passed over before it is verified or decoded.
    fn current_format(session: &Path) -> Result<bool> {
        #[derive(serde::Deserialize)]
        struct Format {
            version: u32,
            ghostty_revision: String,
        }

        let path = session.join(layout::DERIVED).join(INDEX_MANIFEST);
        let bytes = match fs_err::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
            Err(source) => return Err(Error::Filesystem { path, source }),
        };
        let format: Format =
            serde_json::from_slice(&bytes).map_err(|source| return json_error(path, source))?;
        return Ok(
            format.version == STATE_FORMAT_VERSION && format.ghostty_revision == GHOSTTY_REVISION
        );
    }

    fn open_source(
        session: &Path,
        session_manifest_hash: &str,
        final_event_hash: Option<&str>,
    ) -> Result<Option<Self>> {
        if !Self::current_format(session)? {
            return Ok(None);
        }

        let Some(directory) = open_verified_artifact(
            session,
            layout::DERIVED,
            session_manifest_hash,
            final_event_hash,
        )?
        else {
            return Ok(None);
        };
        let manifest_path = directory.join(INDEX_MANIFEST);
        let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
            return Error::Filesystem {
                path: manifest_path.clone(),
                source,
            };
        })?;
        let manifest: ReplayManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|source| return json_error(manifest_path, source))?;

        if manifest.version != STATE_FORMAT_VERSION
            || manifest.ghostty_revision != GHOSTTY_REVISION
            || manifest.state_selection != StateSelection::All
        {
            return Ok(None);
        }
        return Ok(Some(Self {
            directory,
            final_state_id: manifest.final_state_id,
            latest_presented_state_id: manifest.latest_presented_state_id,
            sampled: manifest.sampling_stride_ns.is_some(),
        }));
    }

    /// Path of one verified index member.
    pub(crate) fn member(&self, name: &str) -> PathBuf {
        return self.directory.join(name);
    }

    pub(crate) fn final_state_id(&self) -> u64 {
        return self.final_state_id;
    }

    pub(crate) fn latest_presented_state_id(&self) -> Option<u64> {
        return self.latest_presented_state_id;
    }

    /// Whether the state index lists every boundary state rather than one per sampling stride.
    pub(crate) fn lists_every_state(&self) -> bool {
        return !self.sampled;
    }

    pub(crate) fn checkpoints(&self) -> Result<Vec<SemanticReplayCheckpoint>> {
        let path = self.directory.join(CHECKPOINTS);
        let filesystem_error = |source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        };
        let file = fs_err::File::open(&path).map_err(filesystem_error)?;
        let decoder = zstd::stream::read::Decoder::new(file).map_err(filesystem_error)?;

        let mut checkpoints = Vec::new();

        for line in BufReader::new(decoder).lines() {
            let line = line.map_err(filesystem_error)?;
            checkpoints.push(
                serde_json::from_str(&line)
                    .map_err(|source| return json_error(path.clone(), source))?,
            );
        }

        return Ok(checkpoints);
    }

    pub(crate) fn seed(&self, checkpoint: &SemanticReplayCheckpoint) -> Result<ReplaySeed> {
        return read_checkpoint_frame(&self.directory.join(CHECKPOINT_FRAMES), checkpoint);
    }

    // A checkpoint inside an event at the range start already applied some of its bytes.
    pub(crate) fn seed_before(&self, monotonic_ns: u64) -> Result<Option<ReplaySeed>> {
        return self
            .checkpoints()?
            .iter()
            .rfind(|checkpoint| return checkpoint.monotonic_ns < monotonic_ns)
            .map(|checkpoint| return self.seed(checkpoint))
            .transpose();
    }

    /// The latest checkpoint a replay can start from and still reach `stop`, or the last one
    /// when the replay runs to the end.
    pub(crate) fn seed_for(&self, stop: Option<ReplayStop>) -> Result<Option<ReplaySeed>> {
        return self
            .checkpoints()?
            .iter()
            .rfind(|checkpoint| match stop {
                None => return true,
                Some(ReplayStop::Position(position)) => return checkpoint.state_id <= position,
                Some(ReplayStop::EventSeq(event_seq)) => {
                    return checkpoint
                        .event_seq
                        .is_none_or(|value| return value <= event_seq);
                }
                Some(ReplayStop::MonotonicNs(monotonic_ns)) => {
                    return checkpoint.monotonic_ns <= monotonic_ns;
                }
            })
            .map(|checkpoint| return self.seed(checkpoint))
            .transpose();
    }

    /// Positions and hashes of the listed states, in stream order.
    pub(crate) fn listed_states(&self) -> Result<Vec<(u64, String)>> {
        let path = self.directory.join(STATE_INDEX);
        let file = fs_err::File::open(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let mut listed = Vec::new();

        for line in BufReader::new(file).lines() {
            let line = line.map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
            let entry: StateIndexEntry = serde_json::from_str(&line)
                .map_err(|source| return json_error(path.clone(), source))?;
            listed.push((entry.state_id, entry.state_hash));
        }

        return Ok(listed);
    }
}

fn read_block(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>> {
    let filesystem_error = |source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    };

    let length = usize::try_from(length)
        .map_err(|source| return checkpoint_length_error("frame", source))?;

    let mut file = fs_err::File::open(path).map_err(filesystem_error)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(filesystem_error)?;

    let mut compressed = vec![0_u8; length];
    file.read_exact(&mut compressed).map_err(filesystem_error)?;
    return decode_all(compressed.as_slice()).map_err(filesystem_error);
}

fn read_checkpoint_frame(path: &Path, entry: &SemanticReplayCheckpoint) -> Result<ReplaySeed> {
    let frame = read_block(path, entry.offset, entry.length)?;

    let malformed = || return invalid_replay("replay checkpoint frame is malformed");
    let (record_len, rest) = frame.split_first_chunk::<4>().ok_or_else(malformed)?;
    let record_len = usize::try_from(u32::from_le_bytes(*record_len))
        .map_err(|source| return checkpoint_length_error("record", source))?;
    let (record, checkpoint) = rest.split_at_checked(record_len).ok_or_else(malformed)?;
    let record: StateRecord = serde_json::from_slice(record)
        .map_err(|source| return json_error(path.to_path_buf(), source))?;

    if record.state_id != entry.state_id
        || record.event_seq != entry.event_seq
        || record.event_byte_offset != entry.event_byte_offset
        || record.monotonic_ns != entry.monotonic_ns
        || record.presented != entry.presented
    {
        return Err(invalid_replay(
            "replay checkpoint frame does not match its catalog entry",
        ));
    }
    return Ok(ReplaySeed {
        record,
        checkpoint: checkpoint.to_vec(),
        source_position: entry.source_position.clone(),
    });
}

fn checkpoint_length_error(part: &str, source: TryFromIntError) -> Error {
    return Error::InvalidReplay {
        reason: format!("replay checkpoint {part} length does not fit"),
        source: Some(Box::new(source)),
    };
}

#[derive(Clone, Copy)]
pub(super) struct CheckpointSpacing {
    minimum_units: u64,
    units_per_frame_byte: u64,
}

// Writes engine checkpoints at boundary states, spaced by the stream positions between them.
struct CheckpointWriter {
    catalog: ZstdEncoder<'static, File>,
    frames: BufWriter<File>,
    frames_offset: u64,
    compressor: Compressor<'static>,
    policy: CheckpointSpacing,
    spacing: u64,
    last: Option<u64>,
}

impl CheckpointWriter {
    fn create(directory: &Path, policy: CheckpointSpacing) -> Result<Self> {
        let catalog_path = directory.join(CHECKPOINTS);
        let catalog_error = |source| {
            return Error::Filesystem {
                path: catalog_path.clone(),
                source,
            };
        };
        let mut catalog =
            ZstdEncoder::new(secure_file(&catalog_path)?, CHECKPOINT_COMPRESSION_LEVEL)
                .map_err(catalog_error)?;
        catalog.include_checksum(true).map_err(catalog_error)?;

        let compressor = Compressor::new(CHECKPOINT_COMPRESSION_LEVEL).map_err(|source| {
            return Error::Filesystem {
                path: directory.join(CHECKPOINT_FRAMES),
                source,
            };
        })?;

        return Ok(Self {
            catalog,
            frames: secure_writer(&directory.join(CHECKPOINT_FRAMES))?,
            frames_offset: 0,
            compressor,
            policy,
            spacing: policy.minimum_units,
            last: None,
        });
    }

    fn push(
        &mut self,
        record: &StateRecord,
        source_position: Option<EventLogPosition>,
        engine: &GhosttyVt,
        ranks: &mut TimelineRanks<'_>,
        counts: StateCounts,
        directory: &Path,
    ) -> Result<bool> {
        let due = self.last.is_none_or(|last| {
            return record.state_id.saturating_sub(last) >= self.spacing;
        });

        if !due {
            return Ok(false);
        }

        let Some(checkpoint) = engine.encode_checkpoint()? else {
            tracing::debug!(
                state_id = record.state_id,
                "deferred replay checkpoint inside an over-long unfinished sequence"
            );

            return Ok(false);
        };

        let frames_error = |source| {
            return Error::Filesystem {
                path: directory.join(CHECKPOINT_FRAMES),
                source,
            };
        };

        let record_json = serde_json::to_vec(record)
            .map_err(|source| return json_error(directory.join(CHECKPOINT_FRAMES), source))?;
        let record_len = u32::try_from(record_json.len())
            .map_err(|source| return checkpoint_length_error("record", source))?;

        let mut frame = Vec::with_capacity(4 + record_json.len() + checkpoint.len());
        frame.extend_from_slice(&record_len.to_le_bytes());
        frame.extend_from_slice(&record_json);
        frame.extend_from_slice(&checkpoint);
        let compressed = self.compressor.compress(&frame).map_err(frames_error)?;
        self.frames.write_all(&compressed).map_err(frames_error)?;

        let length = u64::try_from(compressed.len()).unwrap_or(u64::MAX);
        let offset = self.frames_offset;
        self.frames_offset = offset
            .checked_add(length)
            .ok_or_else(|| return invalid_replay("replay checkpoint frame offset overflow"))?;

        // Seeks replay from a checkpoint, so larger frames earn longer stretches between them.
        self.spacing = length
            .saturating_mul(self.policy.units_per_frame_byte)
            .max(self.policy.minimum_units);

        self.last = Some(record.state_id);

        let state_key = (
            record.monotonic_ns,
            record.event_seq.unwrap_or(0),
            2,
            record.event_byte_offset.unwrap_or(0),
        );
        let (timeline_rank, control_index_offset, control_rank) =
            ranks.before(state_key, counts.states_before, counts.effects_before)?;
        write_json_line(
            &mut self.catalog,
            &SemanticReplayCheckpoint {
                state_id: record.state_id,
                event_seq: record.event_seq,
                event_byte_offset: record.event_byte_offset,
                monotonic_ns: record.monotonic_ns,
                presented: record.presented,
                state_rank: counts.states_before,
                hidden_before: counts.hidden_before,
                timeline_rank,
                control_index_offset,
                control_rank,
                source_position,
                offset,
                length,
            },
            directory,
        )?;

        return Ok(true);
    }

    fn finish(self, directory: &Path) -> Result<()> {
        let catalog_path = directory.join(CHECKPOINTS);
        let catalog = self.catalog.finish().map_err(|source| {
            return Error::Filesystem {
                path: catalog_path.clone(),
                source,
            };
        })?;
        catalog.sync_data().map_err(|source| {
            return Error::Filesystem {
                path: catalog_path,
                source,
            };
        })?;

        return Ok(finish_writer(
            self.frames,
            &directory.join(CHECKPOINT_FRAMES),
        )?);
    }
}

#[derive(Clone, Copy, Default)]
struct StateCounts {
    states_before: u64,
    effects_before: u64,
    hidden_before: u64,
}

// Lists the retained states: every boundary state for a full index, every presented one for a
// presented index, or the last presented state of each stride when sampling.
struct StateIndexWriter {
    writer: BufWriter<File>,
    selection: StateSelection,
    stride_ns: Option<u64>,
    count: u64,
    final_hash: Option<String>,
    pending: Option<StateIndexEntry>,
    bucket: Option<u64>,
}

impl StateIndexWriter {
    fn create(directory: &Path, selection: StateSelection, stride_ns: Option<u64>) -> Result<Self> {
        return Ok(Self {
            writer: secure_writer(&directory.join(STATE_INDEX))?,
            selection,
            stride_ns,
            count: 0,
            final_hash: None,
            pending: None,
            bucket: None,
        });
    }

    fn push(&mut self, entry: StateIndexEntry, directory: &Path) -> Result<()> {
        let Some(stride_ns) = self.stride_ns else {
            if self.selection == StateSelection::Presented && !entry.presented {
                return Ok(());
            }
            return self.write(&entry, directory);
        };

        if !entry.presented {
            return Ok(());
        }

        let bucket = entry.monotonic_ns / stride_ns;

        if self.count == 0 && self.pending.is_none() && entry.event_seq.is_none() {
            self.bucket = Some(bucket);

            return self.write(&entry, directory);
        }

        if self.bucket != Some(bucket) {
            self.flush(directory)?;
            self.bucket = Some(bucket);
        }

        self.pending = Some(entry);
        return Ok(());
    }

    fn flush(&mut self, directory: &Path) -> Result<()> {
        return self
            .pending
            .take()
            .map_or(Ok(()), |pending| return self.write(&pending, directory));
    }

    fn write(&mut self, entry: &StateIndexEntry, directory: &Path) -> Result<()> {
        write_json_line(&mut self.writer, entry, directory)?;
        self.final_hash = Some(entry.state_hash.clone());
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("retained semantic state count overflow"))?;

        return Ok(());
    }

    fn finish(mut self, directory: &Path) -> Result<(u64, Option<String>)> {
        self.flush(directory)?;
        finish_writer(self.writer, &directory.join(STATE_INDEX))?;

        return Ok((self.count, self.final_hash));
    }
}

/// Write and catalog a session's default index for tests that exercise the trust rule.
#[cfg(test)]
pub(crate) fn write_cataloged(session: &Path, log: &EventLog) -> Result<PathBuf> {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    fs_err::set_permissions(session, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let directory = SessionLayout::new(session).derived();
    create_private_directory(&directory)?;
    write(&directory, log, None, true, StateSelection::All)?;
    let manifest_bytes = fs_err::read(directory.join(INDEX_MANIFEST)).map_err(|source| {
        return Error::Filesystem {
            path: directory.join(INDEX_MANIFEST),
            source,
        };
    })?;

    append_artifact_root(
        session,
        &directory,
        "semantic_index",
        &manifest_bytes,
        &log.header.session_manifest_hash,
        log.events.last().map(|record| return record.hash.as_str()),
        None,
    )?;
    return Ok(directory);
}

#[cfg(test)]
pub(crate) fn write(
    directory: &Path,
    log: &EventLog,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
) -> Result<(ReplayArtifacts, u64)> {
    return write_source_index(
        directory,
        &IndexSource::Memory(log),
        source_range,
        source_complete,
        state_selection,
        None,
        TEST_CHECKPOINT_SPACING,
    );
}

#[cfg(test)]
pub(crate) fn write_index_with_stride(
    directory: &Path,
    log: &EventLog,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
    sampling_stride_ns: Option<u64>,
) -> Result<(ReplayArtifacts, u64)> {
    return write_source_index(
        directory,
        &IndexSource::Memory(log),
        source_range,
        source_complete,
        state_selection,
        sampling_stride_ns,
        TEST_CHECKPOINT_SPACING,
    );
}

fn write_source_index(
    directory: &Path,
    source: &IndexSource<'_>,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
    sampling_stride_ns: Option<u64>,
    checkpoint_spacing: CheckpointSpacing,
) -> Result<(ReplayArtifacts, u64)> {
    write_control_index(directory, source, source_range)?;
    let mut ranks = TimelineRanks::open(directory, source.records()?, source_range)?;
    let mut checkpoints = (state_selection == StateSelection::All)
        .then(|| return CheckpointWriter::create(directory, checkpoint_spacing))
        .transpose()?;

    let mut index = StateIndexWriter::create(directory, state_selection, sampling_stride_ns)?;
    let mut counts = StateCounts::default();
    let mut latest_presented_state_id = None;
    let last_position = RefCell::new(None);
    let pending_resizes = RefCell::new(Vec::<(Uuid, Option<EventLogPosition>)>::new());
    let mut records = source.positioned_records()?.map(|record| {
        return record.map(|(record, position)| {
            match &record.event {
                Event::ResizeIntent { operation_id, .. } => {
                    pending_resizes
                        .borrow_mut()
                        .push((*operation_id, position.clone()));
                }
                Event::ResizeResult { operation_id, .. } => {
                    pending_resizes
                        .borrow_mut()
                        .retain(|(pending, _)| return pending != operation_id);
                }
                _ => {}
            }

            *last_position.borrow_mut() = position;

            return record;
        });
    });
    let artifacts = replay_for_index(
        source.header(),
        records.by_ref(),
        source_range,
        |visit, engine| {
            let record = match visit {
                ReplayVisit::State(record, _) => record,
                ReplayVisit::Effect(_) => {
                    counts.effects_before = counts.effects_before.saturating_add(1);
                    return Ok(());
                }
            };

            let checkpoint = match checkpoints.as_mut() {
                Some(checkpoints) => {
                    // Replay resumes at the record that produced the state, which is the one read last,
                    // or at an earlier resize intent whose result has not been read yet, so that
                    // resuming sees the intent its result needs.
                    let position = last_position
                        .borrow()
                        .clone()
                        .filter(|position| {
                            return Some(position.next_event_seq) == record.event_seq;
                        })
                        .and_then(|position| {
                            return match pending_resizes.borrow().first() {
                                Some((_, intent)) => intent.clone(),
                                None => Some(position),
                            };
                        });

                    checkpoints.push(&record, position, engine, &mut ranks, counts, directory)?
                }
                None => false,
            };

            counts.states_before = counts.states_before.saturating_add(1);

            if record.presented {
                latest_presented_state_id = Some(record.state_id);
            } else {
                counts.hidden_before = counts.hidden_before.saturating_add(1);
            }

            return index.push(
                StateIndexEntry {
                    state_id: record.state_id,
                    state_hash: record.state_hash,
                    event_seq: record.event_seq,
                    event_byte_offset: record.event_byte_offset,
                    monotonic_ns: record.monotonic_ns,
                    presented: record.presented,
                    checkpoint,
                },
                directory,
            );
        },
    )?;

    // Range replay can stop early; verify that its pass still reaches the same
    // complete canonical prefix before publishing any derived artifact.
    for record in records {
        drop(record?);
    }

    if let Some(checkpoints) = checkpoints {
        checkpoints.finish(directory)?;
    }

    let (retained_state_count, final_retained_state_hash) = index.finish(directory)?;
    write_secure(&directory.join("final.txt"), &artifacts.final_text)?;
    write_secure(&directory.join("final.vt"), &artifacts.final_vt)?;
    write_secure(&directory.join("final.html"), &artifacts.final_html)?;

    let members = collect_artifact_members(directory)?;
    let manifest = ReplayManifest {
        version: STATE_FORMAT_VERSION,
        session_id: source.header().session_id,
        source_event_hash: source.final_hash().map(str::to_owned),
        source_range,
        source_complete,
        state_selection,
        sampling_stride_ns,
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_count: retained_state_count,
        final_state_id: artifacts.final_state_id,
        latest_presented_state_id,
        final_state_hash: artifacts.final_state_hash.clone(),
        final_retained_state_hash,
        members,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| return json_error(directory.join(INDEX_MANIFEST), source))?;
    write_secure(&directory.join(INDEX_MANIFEST), &manifest_bytes)?;
    verify_artifact_members(directory, &manifest.members)?;
    sync_directory(directory)?;

    return Ok((artifacts, retained_state_count));
}

fn write_json_line(writer: &mut impl Write, value: &impl Serialize, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| return json_error(path.to_path_buf(), source))?;

    return writer.write_all(b"\n").map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}
