use super::super::client_error;
use super::super::{EventLog, EventProjection, Result, Session};
use crate::defaults;
use termtrace_control::Reached;
use termtrace_model::{
    execution::{layout::SessionLayout, session::SessionCompletion},
    protocol::control::{ControlCommand, ControlPayload, LiveEventCursor},
};

/// Where the live actor's event file stands, or the completion of an ended session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FollowPosition {
    /// The live actor answered.
    Live {
        /// Device of the writer's open file descriptor.
        device: u64,
        /// Inode of the writer's open file descriptor.
        inode: u64,
        /// Canonical header hash retained by the writer.
        header_hash: String,
        /// Requested position, if still retained and durable.
        start: Option<LiveEventCursor>,
        /// Latest data-synchronized position.
        durable: LiveEventCursor,
    },
    /// The session has completed, before the request or within the completion grace.
    Completed(Box<SessionCompletion>),
}

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

    /// Ask the live actor where its event file stands for following from `next_event_seq`.
    ///
    /// # Errors
    /// Returns the transport failure, the actor's rejection, or a completion-evidence failure.
    pub fn follow(&self, next_event_seq: u64) -> Result<FollowPosition> {
        let reached = self.session.reach_or_completed(
            ControlCommand::EventCursor { next_event_seq },
            defaults::COMPLETION_GRACE,
        )?;

        return match reached {
            Reached::Live(payload) => match *payload {
                ControlPayload::EventCursor {
                    device,
                    inode,
                    header_hash,
                    start,
                    durable,
                } => Ok(FollowPosition::Live {
                    device,
                    inode,
                    header_hash,
                    start,
                    durable,
                }),
                _ => Err(client_error(
                    "actor returned the wrong event cursor payload",
                )),
            },
            Reached::Completed(completion) => Ok(FollowPosition::Completed(completion)),
        };
    }
}
