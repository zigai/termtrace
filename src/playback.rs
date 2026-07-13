//! Timed canonical-event playback for the native Ghostty capture process.

use std::{
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::{Event, OperationStatus},
    event_log::EventLog,
};

/// Clock handshake written before playback begins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaybackTiming {
    /// Wall-clock time corresponding to event monotonic time zero.
    pub replay_origin_unix_ns: u128,
    /// Playback speed multiplier (`1` for native timing).
    pub speed: u32,
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
    /// Delay before event time zero.
    pub start_delay: Duration,
    /// Time to retain the final native frame.
    pub final_hold: Duration,
}

/// Replay canonical output and successful resizes to standard output.
///
/// # Errors
///
/// Returns an error for invalid timing, event-log validation, output, or
/// handshake persistence failures.
pub fn playback(options: &PlaybackOptions) -> Result<()> {
    let log = EventLog::read(options.session_directory.join("events.jsonl"))?;
    let monotonic_origin = Instant::now() + options.start_delay;
    let replay_origin_unix_ns = unix_now_ns()?
        .checked_add(options.start_delay.as_nanos())
        .ok_or_else(|| Error::InvalidReplay {
            reason: "playback wall-clock origin overflow".to_owned(),
        })?;
    let timing = PlaybackTiming {
        replay_origin_unix_ns,
        speed: 1,
    };
    write_create_new_json(&options.timing_file, &timing)?;
    sleep_until(monotonic_origin);

    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut resize_intents = std::collections::HashMap::new();
    for record in &log.events {
        let target = monotonic_origin + Duration::from_nanos(record.monotonic_ns);
        sleep_until(target);
        match &record.event {
            Event::Output { data } => {
                output
                    .write_all(data.as_bytes())
                    .and_then(|()| output.flush())
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
                        .and_then(|()| output.flush())
                        .map_err(output_error)?;
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }
    write_create_new(&options.ready_file, b"ready\n")?;
    thread::sleep(options.final_hold);
    Ok(())
}

fn sleep_until(target: Instant) {
    if let Some(remaining) = target.checked_duration_since(Instant::now()) {
        thread::sleep(remaining);
    }
}

fn unix_now_ns() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|_| Error::ClockBeforeEpoch)
}

fn write_create_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let encoded = serde_json::to_vec(value).map_err(|source| Error::InvalidJson {
        path: path.to_path_buf(),
        json_path: "$".to_owned(),
        source,
    })?;
    write_create_new(path, &encoded)
}

fn write_create_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_data())
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

fn output_error(source: std::io::Error) -> Error {
    Error::Filesystem {
        path: PathBuf::from("<playback-stdout>"),
        source,
    }
}
