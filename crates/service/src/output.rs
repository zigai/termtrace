//! Bounded raw child PTY output streaming, cursors, and wait operations.

use crate::error::{Error, Result};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_engine::inspection::wait_for_output;
use termtrace_evidence::{event_log::EventLogCursor, session::read_manifest};
use termtrace_model::{execution::event::Event, terminal::inspection::WaitOutcome};

/// Cursor tracking scanned byte position and sequence across output pages.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputCursor {
    /// Canonical session directory path.
    pub session_path: PathBuf,
    /// Scanned byte offset in the raw output stream.
    pub byte_offset: u64,
    /// Last scanned event sequence when known.
    pub event_seq: Option<u64>,
}

/// Bounded slice of raw child PTY output bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputPage {
    /// Raw bytes produced by the child process.
    pub bytes: Vec<u8>,
    /// Resumption cursor at the end of this page.
    pub cursor: OutputCursor,
    /// Whether more bytes remain beyond this page.
    pub has_more: bool,
}

/// Service for bounded raw child output reading and synchronization.
pub struct OutputService;

impl OutputService {
    /// Read one bounded page of raw child output bytes.
    ///
    /// # Errors
    /// Returns an error if the log cannot be opened or decoded.
    pub fn read_page(
        session: &Path,
        cursor: Option<OutputCursor>,
        limit_bytes: usize,
    ) -> Result<OutputPage> {
        let manifest = read_manifest(session)?;
        let log_path = session.join(&manifest.event_log);
        let start_offset = cursor.as_ref().map_or(0, |c| return c.byte_offset);
        let limit = limit_bytes.clamp(1, 4 * 1024 * 1024);
        let mut cursor_reader = EventLogCursor::open_snapshot(&log_path)?;
        let mut current_offset = 0_u64;
        let mut bytes = Vec::new();
        let mut last_event_seq = None;

        while let Some(record) = cursor_reader.next_record()? {
            if let Event::Output { data } = &record.event {
                let chunk = data.as_bytes();
                let chunk_len = chunk.len() as u64;
                let chunk_end = current_offset + chunk_len;

                if chunk_end > start_offset && bytes.len() < limit {
                    let slice_start = if current_offset < start_offset {
                        usize::try_from(start_offset - current_offset).map_err(|source| {
                            return Error::InvalidOperation {
                                reason: format!(
                                    "output cursor offset {start_offset} is not addressable"
                                ),
                                source: Some(Box::new(source)),
                            };
                        })?
                    } else {
                        0
                    };

                    let needed = limit - bytes.len();
                    let slice_end = (slice_start + needed).min(chunk.len());

                    if let Some(slice) = chunk.get(slice_start..slice_end) {
                        bytes.extend_from_slice(slice);
                    }

                    last_event_seq = Some(record.seq);
                }

                current_offset = chunk_end;

                if bytes.len() >= limit {
                    break;
                }
            }
        }

        let next_offset = start_offset + bytes.len() as u64;
        let has_more = next_offset < current_offset;

        return Ok(OutputPage {
            bytes,
            cursor: OutputCursor {
                session_path: session.to_path_buf(),
                byte_offset: next_offset,
                event_seq: last_event_seq,
            },
            has_more,
        });
    }

    /// Wait for exact child bytes to appear in the canonical output stream.
    ///
    /// # Errors
    /// Returns an error if the timeout expires or evidence access fails.
    pub fn wait_for_bytes(session: &Path, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return wait_for_output(session, bytes, timeout).map_err(Error::from);
    }
}
