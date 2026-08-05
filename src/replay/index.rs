use super::*;

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
    let completion_exists = session_directory.join("completion.json").is_file();
    if !completion_exists && !options.live_snapshot {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }
    let live_event_count = if !completion_exists {
        // A successful status request is a durable actor boundary: all earlier
        // buffered output is complete on disk before this process snapshots it.
        // A crashed session has no socket, so fall back to its static valid prefix.
        match request(session_directory, ControlCommand::Status) {
            Ok(ControlData::Status { boundaries, .. }) => Some(boundaries.durable.next_event_seq),
            Ok(_) => {
                return Err(invalid_replay(
                    "live status returned an unexpected control response",
                ));
            }
            Err(error) => {
                tracing::debug!(%error, "live snapshot has no responsive control boundary");
                None
            }
        }
    } else {
        None
    };
    let event_path = session_directory.join("events.jsonl");
    let log = match live_event_count {
        Some(event_count) => EventLog::read_prefix(&event_path, event_count)?,
        None => EventLog::read(&event_path)?,
    };
    let source_complete = if completion_exists {
        validate_completed_log(session_directory, &log)?;
        true
    } else {
        false
    };
    let final_monotonic_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_monotonic_ns,
    )?;
    let mut directory_name = derived_directory_name_for(source_range, options.state_selection);
    if !source_complete {
        let source_hash = log.events.last().map_or_else(
            || return log.header.session_id.simple().to_string(),
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
    let result = write_index(
        &staging,
        &log,
        source_range,
        source_complete,
        options.state_selection,
    )
    .and_then(|(artifacts, retained_state_count)| {
        commit_directory(session_directory, &staging, &destination)?;
        let manifest_path = destination.join("manifest.json");
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
                &log.header.session_manifest_hash,
                log.events.last().map(|record| return record.hash.as_str()),
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
    if result.is_err() && staging.exists() {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| {
            return Error::Filesystem {
                path: staging,
                source,
            };
        });
        return finish_with_cleanup(result, cleanup);
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
        StateSelection::All => DERIVED_DIRECTORY.to_owned(),
        StateSelection::Presented => format!("{DERIVED_DIRECTORY}-presented"),
    };
    return source_range.map_or_else(
        || return base.clone(),
        |range| return format!("{base}-{}", range.artifact_suffix()),
    );
}

fn write_index(
    directory: &Path,
    log: &EventLog,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
) -> Result<(ReplayArtifacts, u64)> {
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
    let mut index = secure_writer(&directory.join("state-index.jsonl"))?;
    let mut retained_state_count = 0_u64;
    let mut final_retained_state_hash = None;
    let mut previous_retained_state = None;
    let artifacts = replay_with(log, source_range, |mut record, state, _| {
        if state_selection == StateSelection::Presented && !record.presented {
            return Ok(());
        }
        if state_selection == StateSelection::Presented {
            record.state_id = retained_state_count;
            record.delta = state_delta(previous_retained_state.as_ref(), state);
            previous_retained_state = Some(state.clone());
        }
        write_json_line(&mut states, &record, directory)?;
        let entry = StateIndexEntry {
            state_id: record.state_id,
            state_hash: record.state_hash.clone(),
            event_seq: record.event_seq,
            event_byte_offset: record.event_byte_offset,
            monotonic_ns: record.monotonic_ns,
            presented: record.presented,
        };
        write_json_line(&mut index, &entry, directory)?;
        final_retained_state_hash = Some(record.state_hash);
        retained_state_count = retained_state_count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("retained semantic state count overflow"))?;
        return Ok(());
    })?;
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
    finish_writer(index, &directory.join("state-index.jsonl"))?;
    write_secure(&directory.join("final.txt"), &artifacts.final_text)?;
    write_secure(&directory.join("final.vt"), &artifacts.final_vt)?;
    write_secure(&directory.join("final.html"), &artifacts.final_html)?;
    let members = collect_artifact_members(directory)?;
    let manifest = ReplayManifest {
        version: STATE_FORMAT_VERSION,
        session_id: log.header.session_id,
        source_event_hash: log.events.last().map(|record| return record.hash.clone()),
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
        .map_err(|source| return json_error(directory.join("manifest.json"), source))?;
    write_secure(&directory.join("manifest.json"), &manifest_bytes)?;
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

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Ok(())) => return Err(primary),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}
