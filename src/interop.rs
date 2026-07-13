//! Optional, non-canonical terminal recording interoperability exports.

use std::{collections::HashMap, path::Path, time::Duration};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    Result,
    artifact::write_secure,
    event::{ByteString, Event, OperationStatus},
    event_log::EventLog,
};

#[derive(Serialize)]
struct CastHeader<'environment> {
    version: u8,
    width: u16,
    height: u16,
    timestamp: u64,
    env: CastEnvironment<'environment>,
}

#[derive(Serialize)]
struct CastEnvironment<'term> {
    term: &'term str,
}

/// Export an asciinema v2 cast without changing canonical evidence.
///
/// Asciicast is UTF-8-only, so invalid PTY bytes are represented with Unicode
/// replacement characters. The canonical event log always retains originals.
///
/// # Errors
///
/// Returns an error when the source is invalid, output exists, JSON encoding
/// fails, or output cannot be durably written.
pub fn export_asciicast(
    session_directory: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<()> {
    let session = session_directory.as_ref();
    let log = EventLog::read(session.join("events.jsonl"))?;
    let mut encoded = Vec::new();
    write_line(
        &mut encoded,
        &CastHeader {
            version: 2,
            width: log.header.cols,
            height: log.header.rows,
            timestamp: u64::try_from(log.header.started_unix_ns / 1_000_000_000)
                .unwrap_or(u64::MAX),
            env: CastEnvironment {
                term: &log.header.term,
            },
        },
        output.as_ref(),
    )?;
    let mut inputs: HashMap<Uuid, ByteString> = HashMap::new();
    let mut resizes = HashMap::new();
    for record in &log.events {
        let seconds = Duration::from_nanos(record.monotonic_ns).as_secs_f64();
        match &record.event {
            Event::Output { data } => write_cast_event(
                &mut encoded,
                seconds,
                "o",
                &String::from_utf8_lossy(data.as_bytes()),
                output.as_ref(),
            )?,
            Event::InputIntent {
                operation_id, data, ..
            } => {
                inputs.insert(*operation_id, data.clone());
            }
            Event::InputResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                if let Some(data) = inputs.remove(operation_id) {
                    write_cast_event(
                        &mut encoded,
                        seconds,
                        "i",
                        &String::from_utf8_lossy(data.as_bytes()),
                        output.as_ref(),
                    )?;
                }
            }
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                ..
            } => {
                resizes.insert(*operation_id, (*cols, *rows));
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                if let Some((cols, rows)) = resizes.remove(operation_id) {
                    write_cast_event(
                        &mut encoded,
                        seconds,
                        "r",
                        &format!("{cols}x{rows}"),
                        output.as_ref(),
                    )?;
                }
            }
            Event::InputResult { operation_id, .. } => {
                inputs.remove(operation_id);
            }
            Event::ResizeResult { operation_id, .. } => {
                resizes.remove(operation_id);
            }
            _ => {}
        }
    }
    return write_secure(output.as_ref(), &encoded);
}

fn write_cast_event(
    output: &mut Vec<u8>,
    seconds: f64,
    code: &str,
    data: &str,
    path: &Path,
) -> Result<()> {
    return write_line(output, &(seconds, code, data), path);
}

fn write_line(output: &mut Vec<u8>, value: &impl Serialize, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *output, value).map_err(|source| {
        return crate::Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    output.push(b'\n');
    return Ok(());
}
