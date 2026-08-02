use super::*;

pub(super) fn run_markers(arguments: MarkersArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    return write_json_line(&list_markers(session, arguments.fps)?);
}

pub(super) fn run_review(arguments: ReviewArgs) -> Result<ExitCode> {
    if arguments.commands {
        return write_json_line(&ReviewerCapabilities::default());
    }
    let session = arguments.session.ok_or_else(|| {
        return miette::miette!("review requires SESSION unless --commands is used");
    })?;
    let session = resolve_session_ref(session)?;
    if let (Some(destination), Some(state_id)) = (&arguments.export_state, arguments.state) {
        return write_json_line(&export_review_state(session, state_id, destination)?);
    }
    if arguments.json {
        if let Some(query) = &arguments.search {
            return write_json_line(&search_review(session, query)?);
        }
        return write_json_line(&summarize_review(
            session,
            arguments.around_marker.as_deref(),
        )?);
    }
    run_reviewer(
        session,
        arguments.around_marker.as_deref(),
        arguments.search.as_deref(),
    )?;
    return Ok(ExitCode::SUCCESS);
}

pub(super) fn run_index(arguments: IndexArgs) -> Result<ExitCode> {
    if !arguments.live_snapshot && !arguments.session.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: arguments.session,
        }
        .into());
    }
    let (start_monotonic_ns, end_monotonic_ns) =
        resolve_cli_range(&arguments.session, &arguments.range)?;
    let state_selection = selection(arguments.presented_only);
    let outcome = index_session_with_options(
        arguments.session,
        IndexOptions {
            start_monotonic_ns,
            end_monotonic_ns,
            live_snapshot: arguments.live_snapshot,
            state_selection,
        },
    )?;
    return write_json_line(&serde_json::json!({
        "directory": outcome.directory,
        "state_count": outcome.state_count,
        "semantic_state_count": outcome.semantic_state_count,
        "source_complete": outcome.source_complete,
        "state_selection": outcome.state_selection,
        "final_state_hash": outcome.final_state_hash,
    }));
}

pub(super) fn run_playback(arguments: PlaybackArgs) -> Result<ExitCode> {
    playback(&PlaybackOptions {
        session_directory: arguments.session,
        timing_file: arguments.timing_file,
        error_file: arguments.error_file,
        geometry_file: arguments.geometry_file,
        ready_file: arguments.ready_file,
        start_delay: Duration::from_millis(arguments.start_delay_ms),
        final_hold: Duration::from_millis(arguments.final_hold_ms),
        start_monotonic_ns: arguments.from_ns,
        end_monotonic_ns: arguments.to_ns,
        endpoint_event_seq: arguments.endpoint_event_seq,
        endpoint_event_byte_offset: arguments.endpoint_event_byte_offset,
        instant: arguments.instant,
    })?;
    return Ok(ExitCode::SUCCESS);
}

pub(super) fn run_inspect(arguments: InspectArgs) -> Result<ExitCode> {
    let selector = match (
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.monotonic_ns,
    ) {
        (Some(state), None, None, None) => StateSelector::StateId(state),
        (None, Some(state_hash), None, None) => StateSelector::StateHash(state_hash),
        (None, None, Some(sequence), None) => StateSelector::EventSeq(sequence),
        (None, None, None, Some(time)) => StateSelector::MonotonicNs(time),
        (None, None, None, None) => StateSelector::Final,
        _ => {
            return Err(miette::miette!(
                "exactly one state selector may be provided"
            ));
        }
    };
    let inspection = inspect_session(arguments.session, selector)?;
    if let Some(row_number) = arguments.row {
        let row = inspection
            .state
            .grid
            .iter()
            .find(|row| return row.row == row_number)
            .ok_or_else(|| miette::miette!("semantic row {row_number} does not exist"))?;
        let record = serde_json::json!({
            "state_id": inspection.record.state_id,
            "event_seq": inspection.record.event_seq,
            "event_byte_offset": inspection.record.event_byte_offset,
            "monotonic_ns": inspection.record.monotonic_ns,
            "state_hash": inspection.record.state_hash,
            "presented": inspection.record.presented,
        });
        return match arguments.format {
            InspectFormat::Json => write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({
                    "record": record,
                    "row": row,
                }))
                .into_diagnostic()?
            )),
            InspectFormat::JsonCompact => write_json_line(&serde_json::json!({
                "record": record,
                "row": row,
            })),
            InspectFormat::Text => write_stdout(format!("{}\n", row_text(row))),
        };
    }
    match arguments.format {
        InspectFormat::Json => {
            return write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&inspection).into_diagnostic()?
            ));
        }
        InspectFormat::JsonCompact => return write_json_line(&inspection),
        InspectFormat::Text => {
            return write_stdout(format!("{}\n", state_text(&inspection.state)));
        }
    }
}

pub(super) fn run_cache(arguments: CacheArgs) -> Result<ExitCode> {
    return match arguments.command {
        CacheCommand::Status(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            write_json_line(&replay_cache_status(&session)?)
        }
        CacheCommand::Prune(arguments) => {
            let session = resolve_session_ref(arguments.session)?;
            write_json_line(&prune_replay_cache(&session)?)
        }
    };
}
