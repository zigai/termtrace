use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use termtrace_engine::timeline::resolve_source_range;
use termtrace_evidence::{event_log::EventLog, session::read_manifest};
use termtrace_model::{
    artifact::playback::{PlaybackFailure, PlaybackTiming},
    execution::event::{Event, OperationStatus},
    execution::session::GeometrySource,
    terminal::replay::SourceRange,
};

use super::terminal::{RawTerminalGuard, TerminalInputDrain, query_terminal, resize_and_query};
use crate::{Error, Result};

/// Configuration for replaying canonical events into a native terminal.
#[derive(Clone, Debug)]
pub struct PlaybackOptions {
    /// Completed session whose canonical event log is replayed.
    pub session_directory: PathBuf,
    /// Create-new JSON file receiving the replay clock and selected source range.
    pub timing_file: PathBuf,
    /// Create-new JSON file receiving a serializable failure when playback aborts.
    pub error_file: PathBuf,
    /// Create-new JSON file receiving the terminal geometry observed after setup.
    pub geometry_file: PathBuf,
    /// Create-new handshake file written after all selected events reach the terminal.
    pub ready_file: PathBuf,
    /// Delay between publishing timing metadata and emitting the first event.
    pub start_delay: Duration,
    /// Time to keep the final frame visible before returning.
    pub final_hold: Duration,
    /// Inclusive lower monotonic timestamp for a time-range replay.
    pub start_monotonic_ns: Option<u64>,
    /// Exclusive upper monotonic timestamp for a time-range replay.
    pub end_monotonic_ns: Option<u64>,
    /// Exact event sequence at which replay stops, instead of using a time range.
    pub endpoint_event_seq: Option<u64>,
    /// Optional byte offset within the endpoint event's output payload.
    pub endpoint_event_byte_offset: Option<u64>,
    /// Whether to emit events immediately instead of preserving recorded timing.
    pub instant: bool,
}

/// Replay the selected canonical event range and publish capture handshake files.
///
/// On failure, this also attempts to create [`PlaybackOptions::error_file`] with the
/// failure description before returning the original error.
pub fn playback(options: &PlaybackOptions) -> Result<()> {
    match playback_inner(options) {
        Ok(()) => return Ok(()),
        Err(error) => {
            let failure = PlaybackFailure {
                message: error.to_string(),
            };
            if let Err(cleanup) = write_create_new_json(&options.error_file, &failure) {
                return Err(Error::CleanupFailed {
                    primary: Box::new(error),
                    cleanup: Box::new(cleanup),
                });
            }
            return Err(error);
        }
    }
}

fn playback_inner(options: &PlaybackOptions) -> Result<()> {
    validate_options(options)?;
    let mut raw = RawTerminalGuard::enter(true)?;
    let result = (|| {
        let manifest = read_manifest(&options.session_directory)?;
        let expected = manifest.geometry;
        let reported = resize_and_query(expected.cols, expected.rows)?;
        if (reported.cols, reported.rows) != (expected.cols, expected.rows) {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "Ghostty applied {}x{} cells instead of recorded {}x{}",
                    reported.cols, reported.rows, expected.cols, expected.rows
                ),
            });
        }
        if manifest.geometry_provenance.source == GeometrySource::GhosttyFit
            && (reported.text_width, reported.text_height)
                != (expected.pixel_width, expected.pixel_height)
        {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "Ghostty text area changed from recorded {}x{} pixels to {}x{} pixels",
                    expected.pixel_width,
                    expected.pixel_height,
                    reported.text_width,
                    reported.text_height
                ),
            });
        }
        write_create_new_json(&options.geometry_file, &reported)?;
        return playback_events(options, raw.is_active());
    })();
    return finish_with_cleanup(result, raw.restore());
}

fn validate_options(options: &PlaybackOptions) -> Result<()> {
    if options.endpoint_event_byte_offset.is_some() && options.endpoint_event_seq.is_none() {
        return Err(Error::InvalidReplay {
            reason: "playback byte endpoint requires an event endpoint".to_owned(),
        });
    }
    if options.endpoint_event_seq.is_some()
        && (options.start_monotonic_ns.is_some() || options.end_monotonic_ns.is_some())
    {
        return Err(Error::InvalidReplay {
            reason: "exact playback endpoints cannot be combined with time ranges".to_owned(),
        });
    }
    return Ok(());
}

fn playback_events(options: &PlaybackOptions, input_is_terminal: bool) -> Result<()> {
    let log = EventLog::read(options.session_directory.join("events.jsonl"))?;
    let final_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_ns,
    )?;
    let mut resize_intents: HashMap<uuid::Uuid, (u16, u16)> = HashMap::new();
    if let Some(range) = source_range {
        let manifest = read_manifest(&options.session_directory)?;
        let mut grid = (manifest.geometry.cols, manifest.geometry.rows);
        let mut output = io::stdout().lock();
        for record in log
            .events
            .iter()
            .take_while(|record| return record.monotonic_ns < range.start_monotonic_ns)
        {
            emit_event(
                &mut output,
                &mut resize_intents,
                &record.event,
                Some(&mut grid),
            )?;
        }
        let reported = query_terminal()?;
        if (reported.cols, reported.rows) != grid {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "Ghostty fast-forward ended at {}x{} cells instead of canonical {}x{}",
                    reported.cols, reported.rows, grid.0, grid.1
                ),
            });
        }
    }
    let mut drain = TerminalInputDrain::start(input_is_terminal)?;
    let result = timed_playback(options, &log, source_range, &mut resize_intents);
    finish_with_cleanup(result, drain.stop())?;
    query_terminal()?;
    write_create_new(&options.ready_file, b"ready\n")?;
    thread::sleep(options.final_hold);
    return Ok(());
}

fn timed_playback(
    options: &PlaybackOptions,
    log: &EventLog,
    source_range: Option<SourceRange>,
    resize_intents: &mut HashMap<uuid::Uuid, (u16, u16)>,
) -> Result<()> {
    let monotonic_origin = Instant::now() + options.start_delay;
    let replay_origin_unix_ns = unix_now_ns()?
        .checked_add(options.start_delay.as_nanos())
        .ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "playback wall-clock origin overflow".to_owned(),
            };
        })?;
    write_create_new_json(
        &options.timing_file,
        &PlaybackTiming {
            replay_origin_unix_ns,
            speed: 1,
            source_range,
            endpoint_event_seq: options.endpoint_event_seq,
            endpoint_event_byte_offset: options.endpoint_event_byte_offset,
            instant: options.instant,
        },
    )?;
    sleep_until(monotonic_origin);
    let mut output = io::stdout().lock();
    for record in &log.events {
        if source_range.is_some_and(|range| return record.monotonic_ns < range.start_monotonic_ns) {
            continue;
        }
        if source_range.is_some_and(|range| return record.monotonic_ns >= range.end_monotonic_ns) {
            break;
        }
        if options
            .endpoint_event_seq
            .is_some_and(|endpoint| return record.seq > endpoint)
        {
            break;
        }
        let relative_ns = source_range.map_or(record.monotonic_ns, |range| {
            return record.monotonic_ns.saturating_sub(range.start_monotonic_ns);
        });
        if !options.instant {
            sleep_until(monotonic_origin + Duration::from_nanos(relative_ns));
        }
        if options.endpoint_event_seq == Some(record.seq) {
            emit_endpoint(
                &mut output,
                resize_intents,
                &record.event,
                options.endpoint_event_byte_offset,
            )?;
            break;
        }
        emit_event(&mut output, resize_intents, &record.event, None)?;
    }
    if let Some(range) = source_range {
        sleep_until(monotonic_origin + Duration::from_nanos(range.duration_ns()));
    }
    return Ok(());
}

fn emit_endpoint(
    output: &mut impl io::Write,
    resize_intents: &mut HashMap<uuid::Uuid, (u16, u16)>,
    event: &Event,
    byte_offset: Option<u64>,
) -> Result<()> {
    if let Event::Output { data } = event {
        let end = byte_offset
            .ok_or_else(|| {
                return Error::InvalidReplay {
                    reason: "output endpoint requires an inclusive byte offset".to_owned(),
                };
            })?
            .checked_add(1)
            .and_then(|value| return usize::try_from(value).ok())
            .ok_or_else(|| {
                return Error::InvalidReplay {
                    reason: "output endpoint byte offset overflows".to_owned(),
                };
            })?;
        let prefix = data.as_bytes().get(..end).ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "output endpoint byte offset exceeds the canonical event".to_owned(),
            };
        })?;
        output
            .write_all(prefix)
            .and_then(|()| return output.flush())
            .map_err(output_error)?;
        return Ok(());
    }
    if byte_offset.is_some() {
        return Err(Error::InvalidReplay {
            reason: "only output endpoints accept a byte offset".to_owned(),
        });
    }
    return emit_event(output, resize_intents, event, None);
}

fn emit_event(
    output: &mut impl io::Write,
    resize_intents: &mut HashMap<uuid::Uuid, (u16, u16)>,
    event: &Event,
    applied_grid: Option<&mut (u16, u16)>,
) -> Result<()> {
    match event {
        Event::Output { data } => output
            .write_all(data.as_bytes())
            .and_then(|()| return output.flush())
            .map_err(output_error)?,
        Event::ResizeIntent {
            operation_id,
            cols,
            rows,
            ..
        } => {
            resize_intents.insert(*operation_id, (*cols, *rows));
        }
        Event::ResizeResult {
            operation_id,
            status: OperationStatus::Applied,
            ..
        } => {
            if let Some((cols, rows)) = resize_intents.remove(operation_id) {
                write!(output, "\x1b[8;{rows};{cols}t")
                    .and_then(|()| return output.flush())
                    .map_err(output_error)?;
                if let Some(grid) = applied_grid {
                    *grid = (cols, rows);
                }
            }
        }
        Event::ResizeResult { operation_id, .. } => {
            resize_intents.remove(operation_id);
        }
        _ => {}
    }
    return Ok(());
}

pub(super) fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

fn sleep_until(target: Instant) {
    if let Some(remaining) = target.checked_duration_since(Instant::now()) {
        thread::sleep(remaining);
    }
}
fn unix_now_ns() -> Result<u128> {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| return duration.as_nanos())
        .map_err(|_| return Error::ClockBeforeEpoch);
}
fn write_create_new_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let encoded = serde_json::to_vec(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return write_create_new(path, &encoded);
}
fn write_create_new(path: &Path, bytes: &[u8]) -> Result<()> {
    termtrace_evidence::artifact::write_secure(path, bytes).map_err(Error::from)
}
fn output_error(source: io::Error) -> Error {
    return Error::Filesystem {
        path: PathBuf::from("<playback-stdout>"),
        source,
    };
}
