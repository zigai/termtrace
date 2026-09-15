//! Bounded canonical event log streaming, paging, and cursors.

use crate::error::Result;
use std::path::{Path, PathBuf};
use termtrace_evidence::{event_log::EventLog, session::read_manifest};
use termtrace_model::execution::event::Event;

/// Resumption cursor binding session identity and scanned event positions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EventCursor {
    /// Canonical session directory.
    pub session_path: PathBuf,
    /// Next event sequence number to read.
    pub next_event_seq: u64,
}

/// Bounded page of canonical event records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventPage {
    /// Ordered event items returned for this page.
    pub events: Vec<Event>,
    /// Resumption cursor for the subsequent page.
    pub cursor: EventCursor,
    /// Whether more events remain beyond this page.
    pub has_more: bool,
}

/// Service for bounded canonical event reading.
pub struct EventsService;

impl EventsService {
    /// Read one bounded page of canonical events.
    ///
    /// # Errors
    /// Returns an error if the event log cannot be opened or decoded.
    pub fn read_page(
        session: &Path,
        cursor: Option<EventCursor>,
        limit: usize,
    ) -> Result<EventPage> {
        let manifest = read_manifest(session)?;
        let log_path = session.join(&manifest.event_log);
        let start_seq = cursor.as_ref().map_or(0, |c| return c.next_event_seq);
        let limit_u16 = u16::try_from(limit.clamp(1, 4096)).unwrap_or(4096);
        let records = EventLog::read_page(&log_path, start_seq, limit_u16)?;
        let next_seq = records.last().map_or(start_seq, |r| return r.seq + 1);
        let has_more = records.len() >= usize::from(limit_u16);
        let events = records.into_iter().map(|r| return r.event).collect();

        return Ok(EventPage {
            events,
            cursor: EventCursor {
                session_path: session.to_path_buf(),
                next_event_seq: next_seq,
            },
            has_more,
        });
    }

    /// Read up to `limit` events from the beginning of the session.
    ///
    /// # Errors
    /// Returns an error if event reading fails.
    pub fn read_events(session: &Path, limit: usize) -> Result<Vec<Event>> {
        let page = Self::read_page(session, None, limit)?;

        return Ok(page.events);
    }
}
