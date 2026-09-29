use super::super::client_error;
use super::super::{EventLog, EventProjection, Result, Session};
use termtrace_model::execution::layout::SessionLayout;

/// Bounded typed canonical event reads.
pub struct Events<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Events<'_> {
    /// Read one bounded projection page and its resume sequence.
    ///
    /// # Errors
    /// Returns an error when `limit` is not 1 through 4096 or the event-log page cannot be read and
    /// validated.
    pub fn page(
        &self,
        from_event_seq: u64,
        limit: u16,
        metadata_only: bool,
    ) -> Result<(Vec<EventProjection>, u64)> {
        if limit == 0 || limit > 4096 {
            return Err(client_error("event limit must be from 1 through 4096"));
        }

        let records = EventLog::read_page(
            SessionLayout::new(self.session.path()).event_log(),
            from_event_seq,
            limit,
        )?;

        let events = records
            .iter()
            .map(|record| return EventProjection::from_record(record, metadata_only))
            .collect::<Vec<_>>();

        let next = events
            .last()
            .map_or(from_event_seq, |event| return event.seq.saturating_add(1));

        return Ok((events, next));
    }
}
