//! Bounded canonical event log streaming, paging, and source-bound cursors.

use crate::{error::Result, paging};
use std::path::{Path, PathBuf};
use termtrace_model::execution::{
    event::{event_type, Event, EventProjection},
    paging::{EventPageCursor, ProjectedEventPage},
};

/// Compatibility cursor. New callers should retain the full [`EventPageCursor`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EventCursor {
    /// Session directory; checked even for compatibility reads.
    pub session_path: PathBuf,
    /// Next event sequence number to read.
    pub next_event_seq: u64,
}

/// Compatibility page retaining the original event-only shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventPage {
    /// Ordered event items returned for this page.
    pub events: Vec<Event>,
    /// Resumption cursor for the subsequent page.
    pub cursor: EventCursor,
    /// Whether more events remain in this snapshot.
    pub has_more: bool,
}

/// Service for bounded canonical event reading.
pub struct EventsService;

/// Canonical event type names accepted by event filters.
pub const EVENT_TYPES: &[&str] = &[
    "output",
    "input_intent",
    "input_result",
    "resize_intent",
    "resize_result",
    "signal_intent",
    "signal_result",
    "marker",
    "application",
    "query_response",
    "lifecycle",
    "exit",
    "host_error",
];

impl EventsService {
    /// Read projections with a source- and chain-bound resume cursor and exact lookahead.
    /// Filters select stable canonical event variant names; empty means all events.
    ///
    /// # Errors
    /// Rejects invalid limits/filters, foreign or modified cursors, invalid complete records,
    /// and source replacement/truncation. A partial trailing record is explicitly excluded.
    pub fn page(
        session: &Path,
        cursor: Option<EventPageCursor>,
        limit: u16,
        metadata_only: bool,
        event_types: &[String],
    ) -> Result<ProjectedEventPage> {
        return Self::scan(session, cursor, 0, limit, metadata_only, event_types);
    }

    pub(crate) fn scan(
        session: &Path,
        cursor: Option<EventPageCursor>,
        legacy_start: u64,
        limit: u16,
        metadata_only: bool,
        event_types: &[String],
    ) -> Result<ProjectedEventPage> {
        if !(1..=4096).contains(&limit) {
            return Err(paging::invalid("event limit must be from 1 through 4096"));
        }

        if event_types
            .iter()
            .any(|kind| return !EVENT_TYPES.contains(&kind.as_str()))
        {
            return Err(paging::invalid("unknown canonical event filter"));
        }

        let (mut reader, source) = paging::open(session)?;

        if let Some(cursor) = &cursor {
            paging::check_source(&source, cursor)?;
        }

        let from = cursor
            .as_ref()
            .map_or(legacy_start, |cursor| return cursor.next_event_seq);

        let mut position = paging::start(&source);
        let mut resume = position.clone();
        let mut checked = cursor.is_none();
        let mut events = Vec::new();
        let mut has_more = false;
        let mut retained_bytes = 0_u64;

        while let Some(record) = reader.next_record()? {
            if position.next_event_seq == from {
                if let Some(expected) = &cursor {
                    paging::check_boundary(expected, &position)?;
                }

                checked = true;
            }

            if record.seq >= from && !has_more {
                let matches = event_types.is_empty()
                    || event_types
                        .iter()
                        .any(|kind| return kind == event_type(&record.event));

                let bytes = record.event.decoded_payload_bytes();

                if matches
                    && (events.len() == usize::from(limit)
                        || (!events.is_empty()
                            && retained_bytes.saturating_add(bytes) > 8 * 1024 * 1024))
                {
                    // Keep the cursor immediately before the first unreturned match.
                    has_more = true;
                } else {
                    if matches {
                        retained_bytes = retained_bytes.saturating_add(bytes);
                        events.push(EventProjection::from_record(&record, metadata_only));
                    }

                    paging::next(&mut resume, record.seq, record.hash.clone())?;
                }
            }

            paging::next(&mut position, record.seq, record.hash)?;
        }

        if position.next_event_seq == from {
            if let Some(expected) = &cursor {
                paging::check_boundary(expected, &position)?;
            }

            checked = true;

            if events.is_empty() {
                resume = position.clone();
            }
        }

        if !checked {
            return Err(paging::invalid(
                "event cursor is beyond the validated snapshot",
            ));
        }

        if cursor.is_none() && from > position.next_event_seq {
            // Preserve the historical integer seek position without minting a false bound cursor.
            return Err(paging::invalid(
                "event offset is beyond the validated snapshot",
            ));
        }

        let incomplete_tail = reader.incomplete_line().is_some();
        let _pending = reader.finish_snapshot()?;

        return Ok(ProjectedEventPage {
            events,
            cursor: resume,
            has_more,
            incomplete_tail,
        });
    }

    /// Compatibility event-only page, implemented by the same validated scanner.
    ///
    /// # Errors
    /// Rejects a foreign path, invalid limit, or invalid canonical evidence.
    pub fn read_page(
        session: &Path,
        cursor: Option<EventCursor>,
        limit: usize,
    ) -> Result<EventPage> {
        if let Some(cursor) = &cursor
            && paging::canonical_session(&cursor.session_path)?
                != paging::canonical_session(session)?
        {
            return Err(paging::invalid(
                "event cursor belongs to another session path",
            ));
        }

        let from = cursor.map_or(0, |cursor| return cursor.next_event_seq);
        let limit = u16::try_from(limit)
            .map_err(|_overflow| return paging::invalid("event limit overflow"))?;
        let page = Self::scan(session, None, from, limit, false, &[])?;

        return Ok(EventPage {
            events: page
                .events
                .into_iter()
                .filter_map(|projection| return projection.event)
                .collect(),
            cursor: EventCursor {
                session_path: session.to_path_buf(),
                next_event_seq: page.cursor.next_event_seq,
            },
            has_more: page.has_more,
        });
    }

    /// Read up to `limit` canonical events from the beginning.
    ///
    /// # Errors
    /// Returns the same validation failures as [`Self::read_page`].
    pub fn read_events(session: &Path, limit: usize) -> Result<Vec<Event>> {
        return Ok(Self::read_page(session, None, limit)?.events);
    }
}
