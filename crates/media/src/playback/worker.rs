use super::{
    runtime::{emit_endpoint, emit_event},
    terminal::{query, resize_and_query, RawTerminalGuard, TerminalInputDrain},
};
use crate::{error::finish_with_cleanup, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    os::unix::{
        fs::PermissionsExt as _,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::EventLogCursor,
    session::{canonical_manifest_bytes, manifest_hash, read_manifest},
};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::{
    execution::event::Event, terminal::replay::ReplayEndpoint,
    value::geometry::TerminalGeometryReport,
};
use termtrace_process::poll_until;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerRequest {
    Advance { endpoint: ReplayEndpoint },
    Close,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerResponse {
    Ready {
        endpoint: ReplayEndpoint,
        geometry: TerminalGeometryReport,
    },
    Closed,
    Error {
        message: String,
    },
}

struct PlaybackPosition {
    endpoint: ReplayEndpoint,
    event_hash: Option<String>,
}

/// Serve forward native replay requests inside one owned Ghostty terminal.
///
/// The private socket accepts one parent connection. Closing that connection ends
/// playback. Requests validate an explicit canonical prefix before acknowledging
/// a parser barrier; the caller acquires and labels pixels separately.
///
/// # Errors
/// Returns terminal, protocol, source-validation, or filesystem failures.
pub fn playback_worker(session: &Path, socket: &Path, geometry_file: &Path) -> Result<()> {
    let mut raw = RawTerminalGuard::enter(true)?;
    let result = run_worker(session, socket, geometry_file, raw.is_active());

    return finish_with_cleanup(result, raw.restore());
}

fn run_worker(
    session: &Path,
    socket: &Path,
    geometry_file: &Path,
    terminal_input: bool,
) -> Result<()> {
    let manifest = read_manifest(session)?;
    let manifest_hash = manifest_hash(&canonical_manifest_bytes(&manifest)?);
    let reported = resize_and_query(manifest.geometry.cols, manifest.geometry.rows)?;

    if (reported.cols, reported.rows) != (manifest.geometry.cols, manifest.geometry.rows) {
        return Err(invalid(
            "native replay worker could not establish the canonical grid",
        ));
    }

    let listener =
        UnixListener::bind(socket).map_err(|source| return filesystem(socket, source))?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))
        .map_err(|source| return filesystem(socket, source))?;
    listener
        .set_nonblocking(true)
        .map_err(|source| return filesystem(socket, source))?;

    write_secure(
        geometry_file,
        &serde_json::to_vec(&reported)
            .map_err(|source| return json_error(geometry_file, source))?,
    )?;

    let stream = poll_until(
        Instant::now() + Duration::from_secs(20),
        Duration::from_millis(10),
        || {
            return match listener.accept() {
                Ok((stream, _)) => Ok(Some(stream)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(filesystem(socket, error)),
            };
        },
    )?
    .ok_or_else(|| return filesystem(socket, io::Error::from(io::ErrorKind::TimedOut)))?;

    drop(listener);

    let mut input = BufReader::new(stream);
    let mut last = None;
    let mut resizes = HashMap::new();
    let mut line = Vec::new();

    loop {
        line.clear();
        let count = input
            .by_ref()
            .take(64 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|source| return filesystem(socket, source))?;
        if count == 0 {
            return Ok(());
        }

        if count > 64 * 1024 || line.last() != Some(&b'\n') {
            return Err(invalid("native worker request exceeds its framing limit"));
        }

        let request: WorkerRequest =
            serde_json::from_slice(&line).map_err(|source| return json_error(socket, source))?;
        let endpoint = match request {
            WorkerRequest::Close => {
                write_response(input.get_mut(), &WorkerResponse::Closed, socket)?;

                return Ok(());
            }
            WorkerRequest::Advance { endpoint } => endpoint,
        };

        if endpoint.session_id != manifest.session_id
            || endpoint.session_manifest_hash != manifest_hash
        {
            return Err(invalid(
                "native worker endpoint has a different source session",
            ));
        }

        let result = advance(
            session,
            &endpoint,
            last.as_ref(),
            &mut resizes,
            terminal_input,
        );
        match result {
            Ok((position, geometry)) => {
                write_response(
                    input.get_mut(),
                    &WorkerResponse::Ready { endpoint, geometry },
                    socket,
                )?;

                last = Some(position);
            }
            Err(error) => {
                write_response(
                    input.get_mut(),
                    &WorkerResponse::Error {
                        message: error.to_string(),
                    },
                    socket,
                )?;

                return Err(error);
            }
        }
    }
}

fn advance(
    session: &Path,
    endpoint: &ReplayEndpoint,
    last: Option<&PlaybackPosition>,
    resizes: &mut HashMap<Uuid, (u16, u16)>,
    terminal_input: bool,
) -> Result<(PlaybackPosition, TerminalGeometryReport)> {
    if last.is_some_and(|last| {
        return (endpoint.event_seq, endpoint.event_byte_offset)
            < (last.endpoint.event_seq, last.endpoint.event_byte_offset);
    }) {
        return Err(invalid("native replay worker requires forward endpoints"));
    }

    if endpoint
        .event_seq
        .is_some_and(|seq| return seq >= endpoint.boundary.next_event_seq)
        || endpoint.event_seq.is_none() && endpoint.event_byte_offset.is_some()
    {
        return Err(invalid(
            "native replay endpoint is outside its source prefix",
        ));
    }

    let mut drain = TerminalInputDrain::start(terminal_input)?;
    let result = advance_events(session, endpoint, last, resizes);
    let position = finish_with_cleanup(result, drain.stop())?;
    let geometry = query()?;

    if (geometry.cols, geometry.rows) != (endpoint.cols, endpoint.rows) {
        return Err(invalid(
            "native replay endpoint grid differs from canonical semantics",
        ));
    }
    return Ok((position, geometry));
}

fn advance_events(
    session: &Path,
    endpoint: &ReplayEndpoint,
    last: Option<&PlaybackPosition>,
    resizes: &mut HashMap<Uuid, (u16, u16)>,
) -> Result<PlaybackPosition> {
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    if cursor.header().session_id != endpoint.session_id
        || cursor.header().session_manifest_hash != endpoint.session_manifest_hash
    {
        return Err(invalid(
            "native replay source header differs from its endpoint",
        ));
    }

    let mut output_bytes = 0_u64;
    let mut event_hash = None;
    let mut output = io::stdout().lock();

    while cursor.next_event_seq() < endpoint.boundary.next_event_seq {
        let record = cursor
            .next_record()?
            .ok_or_else(|| return invalid("native replay source prefix is not available"))?;

        if let Event::Output { data } = &record.event {
            output_bytes = output_bytes
                .saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        }

        if let Some(last) = last
            && last.endpoint.event_seq == Some(record.seq)
            && last.event_hash.as_deref() != Some(record.hash.as_str())
        {
            return Err(invalid("previously replayed source bytes changed"));
        }

        let Some(target) = endpoint.event_seq else {
            continue;
        };
        if record.seq > target {
            continue;
        }

        if record.seq == target {
            event_hash = Some(record.hash.clone());
        }

        if let Some(previous) = last.and_then(|last| return last.endpoint.event_seq) {
            if record.seq < previous {
                continue;
            }

            if record.seq == previous {
                if let Event::Output { data } = &record.event {
                    let start = last
                        .and_then(|last| return last.endpoint.event_byte_offset)
                        .and_then(|offset| return offset.checked_add(1))
                        .and_then(|offset| return usize::try_from(offset).ok())
                        .ok_or_else(|| {
                            return invalid("previous native output endpoint has no byte offset");
                        })?;

                    let end = if record.seq == target {
                        endpoint
                            .event_byte_offset
                            .and_then(|offset| return offset.checked_add(1))
                            .and_then(|offset| return usize::try_from(offset).ok())
                            .ok_or_else(|| {
                                return invalid("native output endpoint has no byte offset");
                            })?
                    } else {
                        data.as_bytes().len()
                    };

                    let bytes = data.as_bytes().get(start..end).ok_or_else(|| {
                        return invalid(
                            "native output endpoint moved backward or exceeds its event",
                        );
                    })?;
                    output
                        .write_all(bytes)
                        .and_then(|()| return output.flush())
                        .map_err(|source| {
                            return filesystem(Path::new("<playback-stdout>"), source);
                        })?;
                }

                continue;
            }
        }

        if record.seq == target {
            emit_endpoint(
                &mut output,
                resizes,
                &record.event,
                endpoint.event_byte_offset,
            )?;
        } else {
            emit_event(&mut output, resizes, &record.event, None)?;
        }
    }

    cursor.validate_boundary(
        endpoint.boundary.next_event_seq,
        &endpoint.boundary.event_hash,
    )?;

    if output_bytes != endpoint.boundary.output_bytes {
        return Err(invalid(
            "native replay prefix output-byte count differs from its commitment",
        ));
    }
    return Ok(PlaybackPosition {
        endpoint: endpoint.clone(),
        event_hash,
    });
}

fn write_response(stream: &mut UnixStream, response: &WorkerResponse, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *stream, response)
        .map_err(|source| return json_error(path, source))?;

    return stream
        .write_all(b"\n")
        .map_err(|source| return filesystem(path, source));
}

fn invalid(reason: &str) -> Error {
    return Error::InvalidReplay {
        reason: reason.to_owned(),
        source: None,
    };
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source,
    };
}

fn json_error(path: &Path, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path: PathBuf::from(path),
        json_path: "$".to_owned(),
        source,
    };
}
