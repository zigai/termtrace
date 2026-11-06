use super::{
    build::{replay_timeline_entry, timeline_key},
    selection::range_error,
};
use crate::{
    replay::{
        timeline_event, trust::SourceIdentity, visit_recording_replay_range_while, ValidatedSession,
    },
    Result,
};
use std::{collections::VecDeque, path::Path, sync::atomic::AtomicBool};
use termtrace_evidence::{event_log::EventLogCursor, session::SessionSnapshot};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::{
    execution::event::Event,
    terminal::{
        replay::SourceRange,
        timeline::{TimelineEntry, TimelineItem, TimelinePage},
        vt::VtToken,
    },
};
use termtrace_terminal::VtFramer;

/// Visit an ordered completed timeline with bounded canonical, VT, and semantic cursors.
///
/// Returning false stops after the current entry. Output records remain represented
/// by exact semantic transitions, with optional controls and hidden states.
///
/// # Errors
/// Rejects incomplete or changing evidence, invalid controls, and replay/visitor failures.
pub fn visit_session_timeline_while(
    session: &Path,
    include_hidden: bool,
    include_controls: bool,
    range: Option<SourceRange>,
    visitor: impl FnMut(TimelineEntry) -> Result<bool>,
) -> Result<()> {
    let validated = ValidatedSession::open(session)?;
    visit_recording_timeline_while(
        session,
        true,
        include_hidden,
        include_controls,
        range,
        visitor,
    )?;

    return validated.check();
}

/// Visit an ordered timeline for a verified finite prefix without claiming completion.
///
/// # Errors
/// Rejects modified snapshots, invalid controls, and replay or visitor failures.
pub fn visit_snapshot_timeline_while(
    snapshot: &SessionSnapshot,
    include_hidden: bool,
    include_controls: bool,
    range: Option<SourceRange>,
    visitor: impl FnMut(TimelineEntry) -> Result<bool>,
) -> Result<()> {
    return visit_recording_timeline_while(
        snapshot.path(),
        false,
        include_hidden,
        include_controls,
        range,
        visitor,
    );
}

fn visit_recording_timeline_while(
    session: &Path,
    completed: bool,
    include_hidden: bool,
    include_controls: bool,
    range: Option<SourceRange>,
    mut visitor: impl FnMut(TimelineEntry) -> Result<bool>,
) -> Result<()> {
    let source = SourceIdentity::capture_candidate(session)?;
    let mut canonical = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let mut controls = include_controls
        .then(|| return TokenCursor::new(session, range))
        .transpose()?;

    let mut canonical_next = next_canonical(&mut canonical, range)?;
    let mut control_next = controls
        .as_mut()
        .map(TokenCursor::next)
        .transpose()?
        .flatten();

    let mut stopped = false;
    let mut auxiliary = || -> Result<Option<TimelineEntry>> {
        let use_canonical = match (&canonical_next, &control_next) {
            (Some(canonical), Some(control)) => timeline_key(canonical) <= timeline_key(control),
            (Some(_), None) => true,
            _ => false,
        };

        if use_canonical {
            let entry = canonical_next.take();
            canonical_next = next_canonical(&mut canonical, range)?;

            return Ok(entry);
        }

        let entry = control_next.take();
        control_next = controls
            .as_mut()
            .map(TokenCursor::next)
            .transpose()?
            .flatten();

        return Ok(entry);
    };
    let mut pending = auxiliary()?;

    visit_recording_replay_range_while(
        session,
        completed,
        range,
        &AtomicBool::new(false),
        |visit| {
            let Some(semantic) = replay_timeline_entry(visit, include_hidden) else {
                return Ok(true);
            };

            while pending
                .as_ref()
                .is_some_and(|entry| return timeline_key(entry) <= timeline_key(&semantic))
            {
                let entry = pending
                    .take()
                    .ok_or_else(|| return range_error("timeline cursor lost its pending entry"))?;
                if !visitor(entry)? {
                    stopped = true;

                    return Ok(false);
                }

                pending = auxiliary()?;
            }

            if !visitor(semantic)? {
                stopped = true;

                return Ok(false);
            }
            return Ok(true);
        },
    )?;

    if !stopped {
        while let Some(entry) = pending.take() {
            if !visitor(entry)? {
                break;
            }

            pending = auxiliary()?;
        }
    }

    if source != SourceIdentity::capture_candidate(session)? {
        return Err(range_error(
            "canonical source changed during timeline generation",
        ));
    }
    return Ok(());
}

/// Read one bounded timeline page from a completed session without loading its log.
///
/// # Errors
/// Returns timeline verification errors or rejects a zero limit.
pub fn build_session_timeline_page(
    session: &Path,
    include_hidden: bool,
    include_controls: bool,
    range: Option<SourceRange>,
    offset: u64,
    limit: u16,
) -> Result<TimelinePage> {
    if limit == 0 {
        return Err(range_error("timeline page limit must be greater than zero"));
    }

    let mut entries = Vec::with_capacity(usize::from(limit));
    let mut seen = 0_u64;
    let mut more = false;

    visit_session_timeline_while(session, include_hidden, include_controls, range, |entry| {
        if seen >= offset {
            if entries.len() == usize::from(limit) {
                more = true;

                return Ok(false);
            }

            entries.push(entry);
        }

        seen = seen.saturating_add(1);

        return Ok(true);
    })?;

    let next_offset = more
        .then(|| return offset.saturating_add(u64::try_from(entries.len()).unwrap_or(u64::MAX)));

    return Ok(TimelinePage {
        entries,
        next_offset,
    });
}

fn next_canonical(
    cursor: &mut EventLogCursor,
    range: Option<SourceRange>,
) -> Result<Option<TimelineEntry>> {
    while let Some(record) = cursor.next_record()? {
        if range.is_none_or(|range| return range.contains(record.monotonic_ns))
            && timeline_event(&record.event)
        {
            return Ok(Some(TimelineEntry {
                monotonic_ns: record.monotonic_ns,
                event_seq: Some(record.seq),
                item: TimelineItem::CanonicalEvent {
                    event: record.event,
                },
            }));
        }
    }

    return Ok(None);
}

pub(crate) struct TokenCursor {
    cursor: EventLogCursor,
    framer: Option<VtFramer>,
    pending: VecDeque<VtToken>,
    range: Option<SourceRange>,
}

impl TokenCursor {
    pub(crate) fn new(session: &Path, range: Option<SourceRange>) -> Result<Self> {
        return Ok(Self {
            cursor: EventLogCursor::open(SessionLayout::new(session).event_log())?,
            framer: Some(VtFramer::with_default_bound()?),
            pending: VecDeque::new(),
            range,
        });
    }

    pub(crate) fn next(&mut self) -> Result<Option<TimelineEntry>> {
        loop {
            if let Some(token) = self.pending.pop_front() {
                if self.range.is_some_and(|range| {
                    return !token
                        .source
                        .iter()
                        .any(|source| return range.contains(source.monotonic_ns));
                }) {
                    continue;
                }

                let Some(first) = token.source.first() else {
                    continue;
                };

                return Ok(Some(TimelineEntry {
                    monotonic_ns: first.monotonic_ns,
                    event_seq: Some(first.event_seq),
                    item: TimelineItem::VtToken { token },
                }));
            }

            let Some(framer) = self.framer.as_mut() else {
                return Ok(None);
            };

            match self.cursor.next_record()? {
                Some(record) => {
                    if let Event::Output { data } = &record.event {
                        self.pending.extend(framer.feed(
                            record.seq,
                            record.monotonic_ns,
                            data.as_bytes(),
                        )?);
                    }
                }
                None => {
                    if let Some(framer) = self.framer.take() {
                        self.pending.extend(framer.finish()?);
                    }
                }
            }
        }
    }
}
