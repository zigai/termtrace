use crate::cli::output::ResponseWriter;

use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    process::ExitCode,
    str,
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::{registry::SessionRegistry, timeline::resolve_source_selection};
use termtrace_evidence::event_log::EventLog;
use termtrace_model::{
    protocol::control::{ControlCommand, ControlData},
    registry::RegistryLifecycle,
    terminal::observation::StatePoint,
    value::ByteString,
};
use termtrace_sdk::{control_endpoint_is_reachable, request};

use crate::cli::{analysis::args::TimeRangeArgs, output::write_json_line};

pub(super) struct ResolvedSessionRef {
    pub(super) path: PathBuf,
    pub(super) lifecycle: RegistryLifecycle,
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
    if bytes.contains(&b'/') || bytes.starts_with(b".") {
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
    if bytes.contains(&b'/') || bytes.starts_with(b".") {
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
    if !path.is_dir() {
        return ResolvedSessionRef {
            path,
            lifecycle: RegistryLifecycle::MissingEvidence,
            diagnostic: Some("session directory is missing or unreadable".to_owned()),
        };
    }
    if path.join("completion.json").is_file() {
        return ResolvedSessionRef {
            path,
            lifecycle: RegistryLifecycle::Completed,
            diagnostic: None,
        };
    }
    if !path.join("session.json").is_file() {
        return ResolvedSessionRef {
            path,
            lifecycle: RegistryLifecycle::Incomplete,
            diagnostic: Some("session manifest has not been published".to_owned()),
        };
    }
    if control_endpoint_is_reachable(&path) {
        return ResolvedSessionRef {
            path,
            lifecycle: RegistryLifecycle::Live,
            diagnostic: None,
        };
    }
    return ResolvedSessionRef {
        path,
        lifecycle: RegistryLifecycle::Stale,
        diagnostic: Some(
            "session has no completion evidence and its abstract control endpoint is unreachable"
                .to_owned(),
        ),
    };
}

pub(super) fn resolve_cli_range(
    session: &Path,
    range: &TimeRangeArgs,
) -> Result<(Option<u64>, Option<u64>)> {
    if range.from_marker.is_none() && range.to_marker.is_none() {
        return Ok((range.start_monotonic_ns, range.end_monotonic_ns));
    }
    let event_path = session.join("events.jsonl");
    let live_event_count = if !session.join("completion.json").is_file() {
        match request(session, ControlCommand::Status).into_diagnostic() {
            Ok(ControlData::Status { boundaries, .. }) => Some(boundaries.durable.next_event_seq),
            Ok(_) => {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            }
            Err(error) => {
                tracing::debug!(%error, "marker selection has no responsive control boundary");
                None
            }
        }
    } else {
        None
    };
    let log = match live_event_count {
        Some(event_count) => EventLog::read_prefix(event_path, event_count).into_diagnostic()?,
        None => EventLog::read(event_path).into_diagnostic()?,
    };
    let resolved = resolve_source_selection(
        &log,
        range.start_monotonic_ns,
        range.end_monotonic_ns,
        range.from_marker.as_deref(),
        range.to_marker.as_deref(),
    )
    .into_diagnostic()?;
    return Ok(resolved.map_or((None, None), |value| {
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
