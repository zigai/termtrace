//! Optional, non-canonical asciinema interoperability export.

use std::{collections::HashMap, path::Path, time::Duration};

use serde::Serialize;
use termtrace_evidence::{
    artifact::{finish_writer, secure_writer},
    event_log::EventLog,
};
use termtrace_model::execution::event::{Event, OperationStatus};
use termtrace_model::value::ByteString;
use uuid::Uuid;

use crate::{Error, Result};

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
/// Returns an error when the canonical event log is invalid, JSON encoding
/// fails, the output exists, or the output cannot be durably written.
pub fn export(session_directory: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<()> {
    let session = session_directory.as_ref();
    let output = output.as_ref();
    let log = EventLog::read(session.join("events.jsonl"))?;
    let mut writer = secure_writer(output)?;
    write_line(
        &mut writer,
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
        output,
    )?;
    let mut inputs: HashMap<Uuid, ByteString> = HashMap::new();
    let mut resizes: HashMap<Uuid, (u16, u16)> = HashMap::new();
    for record in &log.events {
        let seconds = Duration::from_nanos(record.monotonic_ns).as_secs_f64();
        match &record.event {
            Event::Output { data } => write_cast_event(
                &mut writer,
                seconds,
                "o",
                String::from_utf8_lossy(data.as_bytes()).as_ref(),
                output,
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
                        &mut writer,
                        seconds,
                        "i",
                        String::from_utf8_lossy(data.as_bytes()).as_ref(),
                        output,
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
                    write_cast_event(&mut writer, seconds, "r", &format!("{cols}x{rows}"), output)?;
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
    finish_writer(writer, output)?;
    return Ok(());
}

fn write_cast_event(
    writer: &mut impl std::io::Write,
    seconds: f64,
    code: &str,
    data: &str,
    path: &Path,
) -> Result<()> {
    return write_line(writer, &(seconds, code, data), path);
}

fn write_line(writer: &mut impl std::io::Write, value: &impl Serialize, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| return Error::json(path, source))?;
    writer
        .write_all(b"\n")
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })?;
    return Ok(());
}
