use crate::cli::output::ResponseWriter;
use crate::cli::{
    analysis::args::{
        CacheArgs, CacheCommand, IndexArgs, InspectArgs, InspectFormat, MarkersArgs, PlaybackArgs,
        PlaybackWorkerArgs, ReviewArgs,
    },
    analysis::reviewer::run as run_reviewer,
    output::{row_text, write_json_line, write_json_pretty, write_stdout},
    session_helpers::{attach_session, resolve_cli_range, resolve_session, resolve_session_path},
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    io::{self, IsTerminal as _},
    process::ExitCode,
    time::Duration,
};
use termtrace_engine::{
    analysis::reviewer::{
        export_state as export_review_state, search as search_review, summarize as summarize_review,
    },
    inspection::inspect_session,
    observation::{prune_replay_cache, replay_cache_status, state_text},
    replay::{index_session_with_options, IndexOptions, StateSelector},
    Error,
};
use termtrace_media::{
    native_capture::validated_visual_manifests,
    playback::{playback, playback_worker, PlaybackOptions},
    presentation::markers as list_markers,
};
use termtrace_model::terminal::{replay::StateSelection, reviewer::ReviewerCapabilities};

pub(in crate::cli) fn run_markers(
    response: &mut ResponseWriter,
    arguments: MarkersArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;

    return write_json_line(
        response,
        &list_markers(session, arguments.fps).into_diagnostic()?,
    );
}

pub(in crate::cli) fn run_review(
    response: &mut ResponseWriter,
    arguments: ReviewArgs,
) -> Result<ExitCode> {
    if arguments.commands {
        return write_json_line(response, &ReviewerCapabilities::default());
    }

    let session = arguments.session.ok_or_else(|| {
        return miette::miette!("review requires SESSION unless --commands is used");
    })?;

    let session = resolve_session_path(session)?;

    if let (Some(destination), Some(state_id)) = (&arguments.export_state, arguments.state) {
        return write_json_line(
            response,
            &export_review_state(session, state_id, destination).into_diagnostic()?,
        );
    }

    if arguments.json {
        if let Some(query) = &arguments.search {
            return write_json_pretty(response, &search_review(session, query).into_diagnostic()?);
        }

        let visual_evidence = validated_visual_manifests(&session).into_diagnostic()?;

        return write_json_pretty(
            response,
            &summarize_review(session, &visual_evidence).into_diagnostic()?,
        );
    }

    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        let visual_evidence = validated_visual_manifests(&session).into_diagnostic()?;
        let summary = summarize_review(&session, &visual_evidence).into_diagnostic()?;
        let text = format!(
            "Session: {}\nStates: {} (presented: {})\nFinal state hash: {}\n",
            session.display(),
            summary.state_count,
            summary.presented_state_count,
            summary.final_state_hash,
        );

        return write_stdout(response, text);
    }

    return run_reviewer(
        session,
        arguments.around_marker.as_deref(),
        arguments.search.as_deref(),
    )
    .into_diagnostic();
}

pub(in crate::cli) fn run_index(
    response: &mut ResponseWriter,
    arguments: IndexArgs,
) -> Result<ExitCode> {
    let session = resolve_session(arguments.session)?.path;
    if !arguments.live_snapshot
        && attach_session(&session)?
            .completion()
            .into_diagnostic()?
            .is_none()
    {
        return Err(Error::SessionIncomplete { session }).into_diagnostic();
    }

    let (start_monotonic_ns, end_monotonic_ns) = resolve_cli_range(&session, &arguments.range)?;

    let state_selection = if arguments.presented_only {
        StateSelection::Presented
    } else {
        StateSelection::All
    };

    let outcome = index_session_with_options(
        session,
        IndexOptions {
            start_monotonic_ns,
            end_monotonic_ns,
            live_snapshot: arguments.live_snapshot,
            state_selection,
            sampling_stride_ns: None,
        },
    )
    .into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "directory": outcome.directory,
            "state_count": outcome.state_count,
            "final_state_id": outcome.final_state_id,
            "source_complete": outcome.source_complete,
            "state_selection": outcome.state_selection,
            "final_state_hash": outcome.final_state_hash,
        }),
    );
}

pub(in crate::cli) fn run_playback(arguments: PlaybackArgs) -> Result<ExitCode> {
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
    })
    .into_diagnostic()?;

    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_playback_worker(arguments: PlaybackWorkerArgs) -> Result<ExitCode> {
    playback_worker(
        &arguments.session,
        &arguments.socket,
        &arguments.geometry_file,
        &arguments.error_file,
    )
    .into_diagnostic()?;

    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_inspect(
    response: &mut ResponseWriter,
    arguments: InspectArgs,
) -> Result<ExitCode> {
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

    let session = resolve_session(arguments.session)?.path;
    let inspection = inspect_session(session, selector).into_diagnostic()?;

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
            InspectFormat::Json => write_stdout(
                response,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "record": record,
                        "row": row,
                    }))
                    .into_diagnostic()?
                ),
            ),
            InspectFormat::JsonCompact => write_json_line(
                response,
                &serde_json::json!({
                    "record": record,
                    "row": row,
                }),
            ),
            InspectFormat::Text => write_stdout(response, format!("{}\n", row_text(row))),
        };
    }

    match arguments.format {
        InspectFormat::Json => {
            return write_stdout(
                response,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&inspection).into_diagnostic()?
                ),
            );
        }
        InspectFormat::JsonCompact => return write_json_line(response, &inspection),
        InspectFormat::Text => {
            return write_stdout(response, format!("{}\n", state_text(&inspection.state)));
        }
    }
}

pub(in crate::cli) fn run_cache(
    response: &mut ResponseWriter,
    arguments: CacheArgs,
) -> Result<ExitCode> {
    return match arguments.command {
        CacheCommand::Status(arguments) => {
            let session = resolve_session_path(arguments.session)?;

            write_json_line(response, &replay_cache_status(&session).into_diagnostic()?)
        }
        CacheCommand::Prune(arguments) => {
            let session = resolve_session_path(arguments.session)?;

            write_json_line(response, &prune_replay_cache(&session).into_diagnostic()?)
        }
    };
}
