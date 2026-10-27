//! Bounded raw child PTY output streaming, source-bound cursors, and byte waits.

use crate::{error::Result, paging};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_engine::inspection::wait_for_output;
use termtrace_model::{
    execution::{
        event::Event,
        paging::{ByteOutputPage, OutputPageCursor},
    },
    terminal::inspection::WaitOutcome,
    value::ByteString,
};

/// Compatibility byte-offset cursor; new callers should retain [`OutputPageCursor`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputCursor {
    /// Canonical session directory path, checked on use.
    pub session_path: PathBuf,
    /// Scanned byte offset in the raw output stream.
    pub byte_offset: u64,
    /// Last output event sequence when known.
    pub event_seq: Option<u64>,
}

/// Compatibility raw child output page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputPage {
    /// Exact emitted bytes.
    pub bytes: Vec<u8>,
    /// Position immediately after the returned bytes.
    pub cursor: OutputCursor,
    /// At least one later byte is present in this snapshot.
    pub has_more: bool,
}

/// Service for canonical output reading and synchronization.
pub struct OutputService;

impl OutputService {
    /// Read a source-bound page with exact snapshot-relative lookahead.
    ///
    /// # Errors
    /// Rejects limits outside 1..=4 MiB, foreign/modified cursors, offsets past the snapshot,
    /// invalid complete records, and source replacement/truncation. Partial tails are explicit.
    pub fn page(
        session: &Path,
        cursor: Option<OutputPageCursor>,
        limit: usize,
    ) -> Result<ByteOutputPage> {
        return Self::scan(session, cursor, 0, limit);
    }

    fn scan(
        session: &Path,
        cursor: Option<OutputPageCursor>,
        legacy_offset: u64,
        limit: usize,
    ) -> Result<ByteOutputPage> {
        if !(1..=4 * 1024 * 1024).contains(&limit) {
            return Err(paging::invalid(
                "output limit must be from 1 through 4194304",
            ));
        }

        let (mut reader, source) = paging::open(session)?;
        if let Some(cursor) = &cursor {
            paging::check_source(&source, &cursor.events)?;
        }

        let start = cursor
            .as_ref()
            .map_or(legacy_offset, |cursor| return cursor.byte_offset);

        let mut position = paging::start(&source);
        let mut resume = cursor.clone().unwrap_or_else(|| {
            return OutputPageCursor {
                events: position.clone(),
                byte_offset: start,
            };
        });

        let mut checked = cursor.is_none();
        if let Some(expected) = &cursor
            && expected.events.next_event_seq == 0
        {
            paging::check_boundary(&expected.events, &position)?;

            if expected.byte_offset != 0 {
                return Err(paging::invalid("nonzero output offset at header"));
            }

            checked = true;
        }

        let mut total = 0_u64;
        let mut bytes = Vec::new();
        let mut legacy_anchor = position.clone();

        while let Some(record) = reader.next_record()? {
            let before = total;
            if let Event::Output { data } = &record.event {
                let chunk = data.as_bytes();
                let length = u64::try_from(chunk.len())
                    .map_err(|_overflow| return paging::invalid("output chunk length overflow"))?;
                total = total
                    .checked_add(length)
                    .ok_or_else(|| return paging::invalid("output offset overflow"))?;

                if total > start && bytes.len() < limit {
                    let skip =
                        usize::try_from(start.saturating_sub(before)).map_err(|_overflow| {
                            return paging::invalid("output offset is not addressable");
                        })?;
                    let available = chunk
                        .get(skip..)
                        .ok_or_else(|| return paging::invalid("invalid output offset"))?;

                    let count = available.len().min(limit - bytes.len());
                    bytes.extend_from_slice(
                        available
                            .get(..count)
                            .ok_or_else(|| return paging::invalid("invalid output slice"))?,
                    );

                    paging::next(&mut resume.events, record.seq, record.hash.clone())?;
                }
            }

            paging::next(&mut position, record.seq, record.hash)?;
            if matches!(record.event, Event::Output { .. }) && total == start && total > before {
                legacy_anchor = position.clone();
            }

            if let Some(expected) = &cursor
                && expected.events.next_event_seq == position.next_event_seq
            {
                paging::check_boundary(&expected.events, &position)?;
                if !matches!(record.event, Event::Output { .. })
                    || expected.byte_offset <= before
                    || expected.byte_offset > total
                {
                    return Err(paging::invalid(
                        "output cursor is not inside its enclosing output event",
                    ));
                }

                checked = true;
            }
        }

        if !checked {
            return Err(paging::invalid(
                "output cursor is beyond the validated snapshot",
            ));
        }

        if start > total {
            return Err(paging::invalid(
                "output offset is beyond the validated snapshot",
            ));
        }

        let next = start
            .checked_add(
                u64::try_from(bytes.len())
                    .map_err(|_overflow| return paging::invalid("output page length overflow"))?,
            )
            .ok_or_else(|| return paging::invalid("output offset overflow"))?;

        // Integer compatibility seeks can begin at EOF; anchor them to the actual last output.
        // Bound cursors from empty reads otherwise retain the exact input position.
        if cursor.is_none() && bytes.is_empty() {
            resume.events = legacy_anchor;
        }

        resume.byte_offset = next;
        let incomplete_tail = reader.incomplete_line().is_some();
        let _pending = reader.finish_snapshot()?;

        return Ok(ByteOutputPage {
            bytes: ByteString::new(bytes),
            cursor: resume,
            has_more: next < total,
            incomplete_tail,
        });
    }

    /// Compatibility page using the same scanner. Integer seeks are not identity-bound cursors.
    ///
    /// # Errors
    /// Rejects foreign paths, invalid limits/offsets, and invalid canonical evidence.
    pub fn read_page(
        session: &Path,
        cursor: Option<OutputCursor>,
        limit_bytes: usize,
    ) -> Result<OutputPage> {
        if let Some(cursor) = &cursor
            && paging::canonical_session(&cursor.session_path)?
                != paging::canonical_session(session)?
        {
            return Err(paging::invalid(
                "output cursor belongs to another session path",
            ));
        }

        let offset = cursor
            .as_ref()
            .map_or(0, |cursor| return cursor.byte_offset);

        let page = Self::scan(session, None, offset, limit_bytes)?;

        return Ok(OutputPage {
            bytes: page.bytes.into_bytes(),
            cursor: OutputCursor {
                session_path: session.to_path_buf(),
                byte_offset: page.cursor.byte_offset,
                event_seq: page.cursor.events.next_event_seq.checked_sub(1),
            },
            has_more: page.has_more,
        });
    }

    /// Wait for exact bytes; text callers must explicitly encode UTF-8.
    ///
    /// # Errors
    /// Returns an error on timeout, completion without a match, or evidence access failure.
    pub fn wait_for_bytes(session: &Path, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return wait_for_output(session, bytes, timeout).map_err(crate::Error::from);
    }
}
