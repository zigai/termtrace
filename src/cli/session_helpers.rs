use crate::cli::output::ResponseWriter;
use crate::cli::{analysis::args::TimeRangeArgs, output::write_json_line};
use miette::{IntoDiagnostic as _, Result};
use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    process::ExitCode,
    str,
    time::Duration,
};
use termtrace_control::{observe_phase, request_or_completed, Reached};
use termtrace_engine::{
    registry::SessionRegistry,
    timeline::{resolve_session_prefix_source_selection, resolve_session_source_selection},
    Result as EngineResult,
};
use termtrace_model::{
    execution::session::SessionPhase,
    protocol::control::{ControlCommand, ControlData},
    terminal::{observation::StatePoint, replay::SourceRange},
    value::ByteString,
};

pub(super) struct ResolvedSessionRef {
    pub(super) path: PathBuf,
    pub(super) lifecycle: SessionPhase,
    pub(super) diagnostic: Option<String>,
}

pub(super) fn resolve_session(target: PathBuf) -> Result<ResolvedSessionRef> {
    let bytes = target.as_os_str().as_bytes();
    if let Some(path) = bytes.strip_prefix(b"path:") {
        if path.is_empty() {
            return Err(miette::miette!("path: session reference is empty"));
        }

        let path = PathBuf::from(OsString::from_vec(path.to_vec()));

        return Ok(resolve_path_lifecycle(path));
    }

    if let Some(name) = bytes.strip_prefix(b"name:") {
        let name = str::from_utf8(name)
            .map_err(|_| miette::miette!("name: session reference must be valid UTF-8"))?;

        let resolved = SessionRegistry::open_default()
            .into_diagnostic()?
            .resolve_lifecycle(name)
            .into_diagnostic()?;

        return Ok(ResolvedSessionRef {
            path: resolved.session,
            lifecycle: resolved.lifecycle,
            diagnostic: resolved.diagnostic,
        });
    }

    if bytes.contains(&b'/') || bytes.starts_with(b".") || target.exists() {
        return Ok(resolve_path_lifecycle(target));
    }

    let name = target
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 session paths require the path: prefix"))?;

    let resolved = SessionRegistry::open_default()
        .into_diagnostic()?
        .resolve_lifecycle(name)
        .into_diagnostic()?;

    return Ok(ResolvedSessionRef {
        path: resolved.session,
        lifecycle: resolved.lifecycle,
        diagnostic: resolved.diagnostic,
    });
}

pub(super) fn resolve_session_path(target: PathBuf) -> Result<PathBuf> {
    let bytes = target.as_os_str().as_bytes();
    if let Some(path) = bytes.strip_prefix(b"path:") {
        if path.is_empty() {
            return Err(miette::miette!("path: session reference is empty"));
        }
        return Ok(PathBuf::from(OsString::from_vec(path.to_vec())));
    }

    if let Some(name) = bytes.strip_prefix(b"name:") {
        let name = str::from_utf8(name)
            .map_err(|_| miette::miette!("name: session reference must be valid UTF-8"))?;

        let entry = SessionRegistry::open_default()
            .into_diagnostic()?
            .resolve(name)
            .into_diagnostic()?;

        return Ok(entry.session_path());
    }

    if bytes.contains(&b'/') || bytes.starts_with(b".") || target.exists() {
        return Ok(target);
    }

    let name = target
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 session paths require the path: prefix"))?;

    let entry = SessionRegistry::open_default()
        .into_diagnostic()?
        .resolve(name)
        .into_diagnostic()?;

    return Ok(entry.session_path());
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
            "lifecycle": resolved.lifecycle.as_str(),
            "session": ByteString::from(resolved.path.as_os_str().as_bytes()),
            "session_display": resolved.path.display().to_string(),
            "diagnostic": resolved.diagnostic,
        }),
    )?;

    return Ok(exit_code);
}

fn resolve_path_lifecycle(path: PathBuf) -> ResolvedSessionRef {
    let observation = observe_phase(&path);

    return ResolvedSessionRef {
        path,
        lifecycle: observation.phase,
        diagnostic: observation.diagnostic,
    };
}

pub(super) fn resolve_cli_range(
    session: &Path,
    range: &TimeRangeArgs,
) -> Result<(Option<u64>, Option<u64>)> {
    if range.from_marker.is_none() && range.to_marker.is_none() {
        return Ok((range.start_monotonic_ns, range.end_monotonic_ns));
    }

    let live_event_count =
        match request_or_completed(session, ControlCommand::Status, Duration::ZERO) {
            Ok(Reached::Completed(_)) => {
                return selection_bounds(resolve_session_source_selection(
                    session,
                    range.start_monotonic_ns,
                    range.end_monotonic_ns,
                    range.from_marker.as_deref(),
                    range.to_marker.as_deref(),
                ));
            }
            Ok(Reached::Live(data)) => {
                let ControlData::Status { boundaries, .. } = *data else {
                    return Err(miette::miette!(
                        "live status returned an unexpected response"
                    ));
                };

                Some(boundaries.durable.next_event_seq)
            }
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
