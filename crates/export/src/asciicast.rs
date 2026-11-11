//! Optional, non-canonical asciinema interoperability export.

use crate::{Error, Result};
use serde::Serialize;
use std::{collections::HashMap, io::Write, path::Path, str, time::Duration};
use termtrace_evidence::{
    artifact::{finish_writer, secure_writer},
    event_log::{EventLogCursor, EventLogSummary},
};
use termtrace_model::execution::event::{Event, OperationStatus};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::value::ByteString;
use uuid::Uuid;

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
    let mut writer = secure_writer(output)?;
    export_to_writer(session, &mut writer, output)?;
    finish_writer(writer, output)?;

    return Ok(());
}

/// Export an asciinema v2 cast to an arbitrary writer.
///
/// # Errors
///
/// Returns an error when the session's event log cannot be read or validated, or writing to
/// `writer` fails.
pub fn export_to_writer(
    session_directory: impl AsRef<Path>,
    mut writer: impl Write,
    display_path: impl AsRef<Path>,
) -> Result<()> {
    let session = session_directory.as_ref();
    let path = display_path.as_ref();
    let log = EventLogSummary::read(SessionLayout::new(session).event_log())?;
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

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
        path,
    )?;

    let mut inputs: HashMap<Uuid, ByteString> = HashMap::new();
    let mut resizes: HashMap<Uuid, (u16, u16)> = HashMap::new();
    let mut output_bytes = Vec::new();
    let mut input_bytes = Vec::new();

    let mut seconds = 0.0;

    while let Some(record) = cursor.next_record()? {
        seconds = Duration::from_nanos(record.monotonic_ns).as_secs_f64();

        match &record.event {
            Event::Output { data } => {
                write_cast_event(
                    &mut writer,
                    &mut output_bytes,
                    seconds,
                    "o",
                    data.as_bytes(),
                    path,
                )?;
            }
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
                        &mut input_bytes,
                        seconds,
                        "i",
                        data.as_bytes(),
                        path,
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
                    write_line(&mut writer, &(seconds, "r", format!("{cols}x{rows}")), path)?;
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

    cursor.finish()?;

    for (code, bytes) in [("o", output_bytes), ("i", input_bytes)] {
        if !bytes.is_empty() {
            write_line(
                &mut writer,
                &(seconds, code, String::from_utf8_lossy(&bytes)),
                path,
            )?;
        }
    }

    return Ok(());
}

fn write_cast_event(
    writer: &mut impl Write,
    pending: &mut Vec<u8>,
    seconds: f64,
    code: &str,
    data: &[u8],
    path: &Path,
) -> Result<()> {
    pending.extend_from_slice(data);
    let incomplete = pending.utf8_chunks().last().map_or(0, |chunk| {
        return if str::from_utf8(chunk.invalid())
            .is_err_and(|error| return error.error_len().is_none())
        {
            chunk.invalid().len()
        } else {
            0
        };
    });

    let complete = pending.len() - incomplete;
    if complete != 0 {
        let (bytes, _) = pending.split_at(complete);
        write_line(
            writer,
            &(seconds, code, String::from_utf8_lossy(bytes)),
            path,
        )?;

        drop(pending.drain(..complete));
    }

    return Ok(());
}

fn write_line(writer: &mut impl Write, value: &impl Serialize, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| return Error::json(path, source))?;

    writer.write_all(b"\n").map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return Ok(());
}
