use super::*;

pub(super) fn resolve_session_ref(target: PathBuf) -> Result<PathBuf> {
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
        return Ok(SessionRegistry::open_default()?
            .resolve(name)?
            .session_path());
    }
    if bytes.contains(&b'/') || bytes.starts_with(b".") {
        return Ok(target);
    }
    let name = target
        .to_str()
        .ok_or_else(|| miette::miette!("non-UTF-8 session paths require the path: prefix"))?;
    return Ok(SessionRegistry::open_default()?
        .resolve(name)?
        .session_path());
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
        match request(session, ControlCommand::Status) {
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
        Some(event_count) => EventLog::read_prefix(event_path, event_count)?,
        None => EventLog::read(event_path)?,
    };
    let resolved = resolve_source_selection(
        &log,
        range.start_monotonic_ns,
        range.end_monotonic_ns,
        range.from_marker.as_deref(),
        range.to_marker.as_deref(),
    )?;
    return Ok(resolved.map_or((None, None), |value| {
        return (Some(value.start_monotonic_ns), Some(value.end_monotonic_ns));
    }));
}

pub(super) fn selection(presented_only: bool) -> StateSelection {
    if presented_only {
        return StateSelection::Presented;
    }
    return StateSelection::All;
}
