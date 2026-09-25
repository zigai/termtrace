use super::{
    engine::validate_generated_replay_seed, invalid_replay, json_error, replay_for_index,
    state_delta, trust::SourceIdentity, ReplaySeed,
};
use crate::{timeline::resolve_source_range, Error, Result};
use serde::Serialize;
use std::{
    env,
    fmt::Write as _,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
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
    execution::layout::{self, SessionLayout},
    terminal::replay::{
        IndexOptions, IndexOutcome, ReplayArtifacts, ReplayManifest, SemanticReplayCheckpoint,
        SourceRange, StateIndexEntry, StateInspection, StateRecord, StateSelection, StateSelector,
        GHOSTTY_REVISION, STATE_FORMAT_VERSION,
    },
    terminal::state::SemanticState,
    value::ByteString,
};
use uuid::Uuid;
use zstd::stream::write::Encoder as ZstdEncoder;

mod control;
mod rank;
mod source;
#[cfg(test)]
mod tests;

use control::write_control_index;
use rank::TimelineRanks;
use source::IndexSource;

const REPLAY_CHECKPOINT_STATE_INTERVAL: u64 = 128;
const INDEX_MANIFEST: &str = "manifest.json";
pub(crate) const STATE_INDEX: &str = "state-index.jsonl";
const CHECKPOINTS: &str = "checkpoints.jsonl.zst";
pub(crate) const CONTROL_INDEX: &str = "control-index.jsonl";
pub(crate) const CONTROL_SOURCE_INDEX: &str = "control-source-index.jsonl";

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
    let completion_exists = session_layout.completion().is_file();

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
    )
    .and_then(|(artifacts, retained_state_count)| {
        if let Some(identity) = identity.as_ref() {
            identity.check(session_directory)?;
        }

        commit_directory(session_directory, &staging, &destination)?;
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
            semantic_state_count: artifacts.state_count,
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
        let Some(directory) = open_verified_artifact(
            session,
            layout::DERIVED,
            &log.header.session_manifest_hash,
            log.events.last().map(|record| return record.hash.as_str()),
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
        return Ok(Some(Self { directory }));
    }

    /// Path of one verified index member.
    pub(crate) fn member(&self, name: &str) -> PathBuf {
        return self.directory.join(name);
    }

    /// Load the latest replay checkpoint at or before `selector`.
    ///
    /// # Errors
    ///
    /// Returns an error when a verified member cannot be read or decoded.
    pub(crate) fn nearest_seed(&self, selector: &StateSelector) -> Result<Option<ReplaySeed>> {
        return load_nearest_replay_seed(&self.directory, selector);
    }
}

fn load_nearest_replay_seed(
    directory: &Path,
    selector: &StateSelector,
) -> Result<Option<ReplaySeed>> {
    let checkpoint_selector = match selector {
        StateSelector::Final => Some(CheckpointSelector::Final),
        StateSelector::LatestPresented => Some(CheckpointSelector::LatestPresented),
        StateSelector::StateId(state_id) => Some(CheckpointSelector::StateId(*state_id)),
        StateSelector::EventSeq(event_seq) => Some(CheckpointSelector::EventSeq(*event_seq)),
        StateSelector::MonotonicNs(monotonic_ns) => {
            Some(CheckpointSelector::MonotonicNs(*monotonic_ns))
        }
        StateSelector::StateHash(state_hash) => {
            let index_path = directory.join(STATE_INDEX);
            let file = fs_err::File::open(&index_path).map_err(|source| {
                return Error::Filesystem {
                    path: index_path.clone(),
                    source,
                };
            })?;

            let mut target_state_id = None;
            for line in BufReader::new(file).lines() {
                let line = line.map_err(|source| {
                    return Error::Filesystem {
                        path: index_path.clone(),
                        source,
                    };
                })?;
                let entry: StateIndexEntry = serde_json::from_str(&line)
                    .map_err(|source| return json_error(index_path.clone(), source))?;
                if entry.state_hash == *state_hash {
                    target_state_id = Some(entry.state_id);
                    break;
                }
            }

            target_state_id.map(CheckpointSelector::StateId)
        }
    };
    let Some(checkpoint_selector) = checkpoint_selector else {
        return Ok(None);
    };

    let checkpoints_path = directory.join(CHECKPOINTS);
    let file = fs_err::File::open(&checkpoints_path).map_err(|source| {
        return Error::Filesystem {
            path: checkpoints_path.clone(),
            source,
        };
    })?;
    let decoder = zstd::stream::read::Decoder::new(file).map_err(|source| {
        return Error::Filesystem {
            path: checkpoints_path.clone(),
            source,
        };
    })?;

    let mut selected = None;
    for line in BufReader::new(decoder).lines() {
        let line = line.map_err(|source| {
            return Error::Filesystem {
                path: checkpoints_path.clone(),
                source,
            };
        })?;
        let checkpoint: SemanticReplayCheckpoint = serde_json::from_str(&line)
            .map_err(|source| return json_error(checkpoints_path.clone(), source))?;
        if checkpoint_precedes_selector(&checkpoint.inspection.record, &checkpoint_selector) {
            selected = Some(ReplaySeed {
                inspection: checkpoint.inspection,
                engine_vt: checkpoint.engine_vt.into_bytes(),
            });
            continue;
        }

        break;
    }

    return Ok(selected);
}

#[derive(Clone, Debug)]
enum CheckpointSelector {
    Final,
    LatestPresented,
    StateId(u64),
    EventSeq(u64),
    MonotonicNs(u64),
}

fn checkpoint_precedes_selector(record: &StateRecord, selector: &CheckpointSelector) -> bool {
    return match selector {
        CheckpointSelector::Final | CheckpointSelector::LatestPresented => true,
        CheckpointSelector::StateId(state_id) => record.state_id <= *state_id,
        CheckpointSelector::EventSeq(event_seq) => record
            .event_seq
            .is_none_or(|value| return value <= *event_seq),
        CheckpointSelector::MonotonicNs(monotonic_ns) => record.monotonic_ns <= *monotonic_ns,
    };
}

/// Write and catalog a session's default index for tests that exercise the trust rule.
#[cfg(test)]
pub(crate) fn write_cataloged_index(session: &Path, log: &EventLog) -> Result<PathBuf> {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    fs_err::set_permissions(session, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let directory = SessionLayout::new(session).derived();
    create_private_directory(&directory)?;
    write_index(&directory, log, None, true, StateSelection::All)?;
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
pub(crate) fn write_index(
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
    );
}

fn write_source_index(
    directory: &Path,
    source: &IndexSource<'_>,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
    sampling_stride_ns: Option<u64>,
) -> Result<(ReplayArtifacts, u64)> {
    write_control_index(directory, source, source_range)?;
    let mut ranks = TimelineRanks::new(directory, source.records()?, source_range)?;
    let states_path = directory.join("states.jsonl.zst");
    let states_file = secure_file(&states_path)?;
    let mut states = zstd::stream::write::Encoder::new(states_file, 3).map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    states.include_checksum(true).map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;

    let checkpoints_path = directory.join(CHECKPOINTS);
    let checkpoints_file = secure_file(&checkpoints_path)?;
    let mut checkpoints =
        zstd::stream::write::Encoder::new(checkpoints_file, 3).map_err(|source| {
            return Error::Filesystem {
                path: checkpoints_path.clone(),
                source,
            };
        })?;
    checkpoints.include_checksum(true).map_err(|source| {
        return Error::Filesystem {
            path: checkpoints_path.clone(),
            source,
        };
    })?;

    let mut index = secure_writer(&directory.join(STATE_INDEX))?;
    let mut retained_state_count = 0_u64;
    let mut final_retained_state_hash = None;
    let mut previous_retained_state = None;
    let mut current_bucket: Option<u64> = None;
    let mut pending_state: Option<(StateRecord, SemanticState)> = None;
    let mut records = source.records()?;

    let commit_retained_state = |mut record: StateRecord,
                                 state: &SemanticState,
                                 checkpoint: bool,
                                 ranks: &mut TimelineRanks,
                                 states: &mut ZstdEncoder<File>,
                                 index: &mut BufWriter<File>,
                                 retained_state_count: &mut u64,
                                 final_retained_state_hash: &mut Option<String>,
                                 previous_retained_state: &mut Option<SemanticState>,
                                 directory: &Path|
     -> Result<()> {
        if state_selection == StateSelection::Presented {
            record.state_id = *retained_state_count;
            record.delta = state_delta(previous_retained_state.as_ref(), state);
            *previous_retained_state = Some(state.clone());
        }

        write_json_line(states, &record, directory)?;

        let state_key = (
            record.monotonic_ns,
            record.event_seq.unwrap_or(0),
            2,
            record.event_byte_offset.unwrap_or(0),
        );
        let (timeline_rank, control_index_offset, control_rank) = if checkpoint {
            let (rank, offset, control_count) = ranks.before(state_key, record.state_id)?;

            (Some(rank), Some(offset), Some(control_count))
        } else {
            (None, None, None)
        };

        let entry = StateIndexEntry {
            state_id: record.state_id,
            state_hash: record.state_hash.clone(),
            event_seq: record.event_seq,
            event_byte_offset: record.event_byte_offset,
            monotonic_ns: record.monotonic_ns,
            presented: record.presented,
            checkpoint,
            timeline_rank,
            control_index_offset,
            control_rank,
        };

        write_json_line(index, &entry, directory)?;
        *final_retained_state_hash = Some(record.state_hash);
        *retained_state_count = retained_state_count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("retained semantic state count overflow"))?;

        return Ok(());
    };

    let artifacts = replay_for_index(
        source.header(),
        records.by_ref(),
        source_range,
        sampling_stride_ns,
        |record, state, engine| {
            let checkpoint_candidate = state_selection == StateSelection::All
                && !state.modes.alternate_screen
                && record.state_id % REPLAY_CHECKPOINT_STATE_INTERVAL == 0;

            let mut checkpoint = false;

            if checkpoint_candidate && let Some(engine_vt) = engine.format_replay_seed()? {
                let inspection = StateInspection {
                    record: record.clone(),
                    state: state.clone(),
                };
                let seed = ReplaySeed {
                    inspection: inspection.clone(),
                    engine_vt: engine_vt.clone(),
                };

                match validate_generated_replay_seed(source.header(), &seed) {
                    Ok(()) => {
                        write_json_line(
                            &mut checkpoints,
                            &SemanticReplayCheckpoint {
                                inspection,
                                engine_vt: ByteString::from(engine_vt),
                            },
                            directory,
                        )?;

                        checkpoint = true;
                    }
                    Err(Error::CheckpointReconstruction { mismatched_fields }) => {
                        tracing::debug!(
                            state_id = record.state_id,
                            ?mismatched_fields,
                            "skipped non-reconstructable semantic replay checkpoint"
                        );
                    }
                    Err(error) => return Err(error),
                }
            }

            if state_selection == StateSelection::Presented && !record.presented {
                return Ok(());
            }

            if let Some(stride_ns) = sampling_stride_ns {
                let bucket = record.monotonic_ns / stride_ns;

                if retained_state_count == 0 && record.event_seq.is_none() {
                    commit_retained_state(
                        record,
                        state,
                        checkpoint,
                        &mut ranks,
                        &mut states,
                        &mut index,
                        &mut retained_state_count,
                        &mut final_retained_state_hash,
                        &mut previous_retained_state,
                        directory,
                    )?;

                    current_bucket = Some(bucket);

                    return Ok(());
                }

                if current_bucket == Some(bucket) {
                    pending_state = Some((record, state.clone()));
                } else {
                    if let Some((pending_rec, pending_st)) = pending_state.take() {
                        commit_retained_state(
                            pending_rec,
                            &pending_st,
                            false,
                            &mut ranks,
                            &mut states,
                            &mut index,
                            &mut retained_state_count,
                            &mut final_retained_state_hash,
                            &mut previous_retained_state,
                            directory,
                        )?;
                    }

                    current_bucket = Some(bucket);
                    pending_state = Some((record, state.clone()));
                }
            } else {
                commit_retained_state(
                    record,
                    state,
                    checkpoint,
                    &mut ranks,
                    &mut states,
                    &mut index,
                    &mut retained_state_count,
                    &mut final_retained_state_hash,
                    &mut previous_retained_state,
                    directory,
                )?;
            }

            return Ok(());
        },
    )?;

    if let Some((pending_rec, pending_st)) = pending_state.take() {
        commit_retained_state(
            pending_rec,
            &pending_st,
            false,
            &mut ranks,
            &mut states,
            &mut index,
            &mut retained_state_count,
            &mut final_retained_state_hash,
            &mut previous_retained_state,
            directory,
        )?;
    }

    // Range replay can stop early; verify that its pass still reaches the same
    // complete canonical prefix before publishing any derived artifact.
    for record in records {
        drop(record?);
    }

    let states_file = states.finish().map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    states_file.sync_data().map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;

    let checkpoints_file = checkpoints.finish().map_err(|source| {
        return Error::Filesystem {
            path: checkpoints_path.clone(),
            source,
        };
    })?;
    checkpoints_file.sync_data().map_err(|source| {
        return Error::Filesystem {
            path: checkpoints_path,
            source,
        };
    })?;

    finish_writer(index, &directory.join(STATE_INDEX))?;
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
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_count: retained_state_count,
        semantic_state_count: artifacts.state_count,
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
