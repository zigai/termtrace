//! Timed canonical-event playback for the native Ghostty capture process.

use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{self, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::{Event, OperationStatus},
    event_log::EventLog,
    geometry::{query_terminal, resize_and_query},
    session::{GeometrySource, read_manifest},
    terminal_mode::{RawTerminalGuard, TerminalInputDrain},
    timeline::{SourceRange, resolve_source_range},
};

/// Clock handshake written before playback begins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaybackTiming {
    /// Wall-clock time corresponding to source-range start (or event time zero).
    pub replay_origin_unix_ns: u128,
    /// Playback speed multiplier (`1` for native timing).
    pub speed: u32,
    /// Selected half-open source interval, absent for full-session playback.
    pub source_range: Option<SourceRange>,
}

/// Configuration for timed playback inside Ghostty.
#[derive(Clone, Debug)]
pub struct PlaybackOptions {
    /// Source session directory.
    pub session_directory: PathBuf,
    /// Handshake JSON path.
    pub timing_file: PathBuf,
    /// File created after all canonical events are emitted.
    pub ready_file: PathBuf,
    /// Delay before source-range start (or event time zero for full playback).
    pub start_delay: Duration,
    /// Time to retain the final native frame.
    pub final_hold: Duration,
    /// Optional inclusive source offset.
    pub start_monotonic_ns: Option<u64>,
    /// Optional exclusive source offset.
    pub end_monotonic_ns: Option<u64>,
}

/// Replay canonical output and successful resizes to standard output.
///
/// # Errors
///
/// Returns an error for invalid timing, event-log validation, output, or
/// handshake persistence failures.
pub fn playback(options: &PlaybackOptions) -> Result<()> {
    let mut terminal_mode = RawTerminalGuard::enter(true)?;
    if let Err(error) = prepare_terminal_geometry(options) {
        return finish_with_cleanup(Err(error), terminal_mode.restore());
    }
    let result = playback_with_raw_input(options, terminal_mode.is_active());
    return finish_with_cleanup(result, terminal_mode.restore());
}

fn prepare_terminal_geometry(options: &PlaybackOptions) -> Result<()> {
    let manifest = read_manifest(&options.session_directory)?;
    let expected = manifest.geometry;
    let reported = resize_and_query(expected.cols, expected.rows)?;
    if reported.cols != expected.cols || reported.rows != expected.rows {
        return Err(Error::InvalidReplay {
            reason: format!(
                "Ghostty applied {}x{} cells instead of recorded {}x{}; increase the capture viewport or use fitted geometry",
                reported.cols, reported.rows, expected.cols, expected.rows
            ),
        });
    }
    if manifest.geometry_provenance.source == GeometrySource::GhosttyFit
        && (reported.text_width != expected.pixel_width
            || reported.text_height != expected.pixel_height)
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
    return Ok(());
}

fn playback_with_raw_input(options: &PlaybackOptions, input_is_terminal: bool) -> Result<()> {
    let log = EventLog::read(options.session_directory.join("events.jsonl"))?;
    let final_monotonic_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_monotonic_ns,
    )?;
    let mut resize_intents = HashMap::new();
    if let Some(range) = source_range {
        let manifest = read_manifest(&options.session_directory)?;
        let mut expected_grid = (manifest.geometry.cols, manifest.geometry.rows);
        {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            for record in log
                .events
                .iter()
                .take_while(|record| return record.monotonic_ns < range.start_monotonic_ns)
            {
                emit_event(
                    &mut output,
                    &mut resize_intents,
                    &record.event,
                    Some(&mut expected_grid),
                )?;
            }
        }
        let reported = query_terminal()?;
        if (reported.cols, reported.rows) != expected_grid {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "Ghostty fast-forward ended at {}x{} cells instead of canonical {}x{}",
                    reported.cols, reported.rows, expected_grid.0, expected_grid.1
                ),
            });
        }
    }

    let mut input_drain = TerminalInputDrain::start(input_is_terminal)?;
    let result = timed_playback(options, &log, source_range, &mut resize_intents);
    return finish_with_cleanup(result, input_drain.stop());
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
    let timing = PlaybackTiming {
        replay_origin_unix_ns,
        speed: 1,
        source_range,
    };
    write_create_new_json(&options.timing_file, &timing)?;
    sleep_until(monotonic_origin);

    let stdout = io::stdout();
    let mut output = stdout.lock();
    for record in &log.events {
        if source_range.is_some_and(|range| return record.monotonic_ns < range.start_monotonic_ns) {
            continue;
        }
        if source_range.is_some_and(|range| return record.monotonic_ns >= range.end_monotonic_ns) {
            break;
        }
        let relative_ns = source_range.map_or(record.monotonic_ns, |range| {
            return record.monotonic_ns.saturating_sub(range.start_monotonic_ns);
        });
        let target = monotonic_origin + Duration::from_nanos(relative_ns);
        sleep_until(target);
        emit_event(&mut output, resize_intents, &record.event, None)?;
    }
    if let Some(range) = source_range {
        sleep_until(monotonic_origin + Duration::from_nanos(range.duration_ns()));
    }
    write_create_new(&options.ready_file, b"ready\n")?;
    thread::sleep(options.final_hold);
    return Ok(());
}

fn emit_event(
    output: &mut impl io::Write,
    resize_intents: &mut HashMap<uuid::Uuid, (u16, u16)>,
    event: &Event,
    applied_grid: Option<&mut (u16, u16)>,
) -> Result<()> {
    match event {
        Event::Output { data } => {
            output
                .write_all(data.as_bytes())
                .and_then(|()| return output.flush())
                .map_err(output_error)?;
        }
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

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(primary), Ok(())) => return Err(primary),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
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

fn write_create_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return file
        .write_all(bytes)
        .and_then(|()| return file.sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
}

fn output_error(source: io::Error) -> Error {
    return Error::Filesystem {
        path: PathBuf::from("<playback-stdout>"),
        source,
    };
}
