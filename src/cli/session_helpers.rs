use crate::cli::output::path_value;
use crate::cli::output::ResponseWriter;
use crate::cli::{analysis::args::TimeRangeArgs, output::write_json_line};
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};
use termtrace_engine::{
    timeline::{resolve_session_prefix_source_selection, resolve_session_source_selection},
    Result as EngineResult,
};
use termtrace_model::terminal::{observation::StatePoint, replay::SourceRange};
pub(super) use termtrace_sdk::session::LocatedSession as ResolvedSessionRef;
use termtrace_sdk::session::{Session, SessionStatus, SessionTarget};

/// Locate a session reference and observe its phase without requiring it to be healthy.
pub(super) fn resolve_session(target: PathBuf) -> Result<ResolvedSessionRef> {
    return SessionTarget::parse(target.as_os_str())
        .and_then(|target| return target.locate())
        .into_diagnostic();
}

/// Resolve a session reference to a healthy canonical session directory.
pub(super) fn resolve_session_path(target: PathBuf) -> Result<PathBuf> {
    return SessionTarget::parse(target.as_os_str())
        .and_then(|target| return target.path())
        .into_diagnostic();
}

/// Attach to a located session through this executable.
pub(super) fn attach_session(path: &Path) -> Result<Session> {
    return Session::attach(path, env::current_exe().into_diagnostic()?).into_diagnostic();
}

pub(super) fn write_session_lifecycle(
    response: &mut ResponseWriter,
    resolved: &ResolvedSessionRef,
    operation: &'static str,
    exit_code: ExitCode,
) -> Result<ExitCode> {
    write_json_line(
        response,
        &serde_json::json!({
            "kind": "session_lifecycle",
            "operation": operation,
            "lifecycle": resolved.phase.as_str(),
            "session": path_value(&resolved.path),
            "diagnostic": resolved.diagnostic,
        }),
    )?;

    return Ok(exit_code);
}

pub(super) fn resolve_cli_range(
    session: &Path,
    range: &TimeRangeArgs,
) -> Result<(Option<u64>, Option<u64>)> {
    if range.from_marker.is_none() && range.to_marker.is_none() {
        return Ok((range.start_monotonic_ns, range.end_monotonic_ns));
    }

    let status = attach_session(session).and_then(|attached| {
        return attached.status().into_diagnostic();
    });

    let live_event_count = match status {
        Ok(SessionStatus::Completed { .. }) => {
            return selection_bounds(resolve_session_source_selection(
                session,
                range.start_monotonic_ns,
                range.end_monotonic_ns,
                range.from_marker.as_deref(),
                range.to_marker.as_deref(),
            ));
        }
        Ok(SessionStatus::Live { boundaries, .. }) => Some(boundaries.durable.next_event_seq),
        Err(error) => {
            tracing::debug!(%error, "marker selection has no responsive control boundary");
            None
        }
    };
    return selection_bounds(resolve_session_prefix_source_selection(
        session,
        live_event_count,
        range.start_monotonic_ns,
        range.end_monotonic_ns,
        range.from_marker.as_deref(),
        range.to_marker.as_deref(),
    ));
}

fn selection_bounds(
    selection: EngineResult<Option<SourceRange>>,
) -> Result<(Option<u64>, Option<u64>)> {
    return Ok(selection.into_diagnostic()?.map_or((None, None), |value| {
        return (Some(value.start_monotonic_ns), Some(value.end_monotonic_ns));
    }));
}

pub(super) fn resolve_state_point(
    state: Option<u64>,
    state_hash: Option<String>,
    event_seq: Option<u64>,
    at: Option<u64>,
    at_marker: Option<String>,
    default: Option<StatePoint>,
    context: &str,
) -> Result<StatePoint> {
    match (state, state_hash, event_seq, at, at_marker) {
        (Some(state_id), None, None, None, None) => return Ok(StatePoint::StateId { state_id }),
        (None, Some(state_hash), None, None, None) => {
            return Ok(StatePoint::StateHash { state_hash });
        }
        (None, None, Some(event_seq), None, None) => return Ok(StatePoint::EventSeq { event_seq }),
        (None, None, None, Some(monotonic_ns), None) => {
            return Ok(StatePoint::MonotonicNs { monotonic_ns });
        }
        (None, None, None, None, Some(label)) => return Ok(StatePoint::Marker { label }),
        (None, None, None, None, None) => {
            return default
                .ok_or_else(|| miette::miette!("exactly one {context} selector is required"));
        }
        _ => {
            return Err(miette::miette!(
                "exactly one {context} selector may be supplied"
            ));
        }
    }
}
