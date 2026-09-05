//! Bounded, cancellable exact semantic pages for the interactive reviewer.

use std::{
    collections::VecDeque,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use termtrace_evidence::event_log::EventLogCursor;
use termtrace_model::{
    execution::event::Event,
    terminal::{observation::StatePoint, replay::StateRecord, state::SemanticState},
};

use super::review_error;
use crate::{
    Result,
    observation::{semantic_state_with_hash_for_session_cancellable, state_text},
    replay::{trust::SourceIdentity, visit_session_states_while},
};

/// Maximum distinct semantic entries retained by a reviewer page.
pub const REVIEW_PAGE_STATES: usize = 64;
const MAX_PAGE_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Safe-to-render semantic content, independent of a replay engine's lifetime.
#[derive(Clone, Debug)]
pub struct ReviewViewport {
    /// Exact plain semantic viewport; the terminal renderer must neutralize controls.
    pub text: String,
    /// Semantic cursor row.
    pub cursor_row: Option<u16>,
    /// Semantic cursor column.
    pub cursor_column: Option<u16>,
    /// Whether the semantic cursor is visible.
    pub cursor_visible: bool,
}

impl ReviewViewport {
    /// Copy only the viewport required by the reviewer, not the full semantic grid.
    pub fn from_state(state: &SemanticState) -> Self {
        return Self {
            text: state_text(state),
            cursor_row: state.cursor.row,
            cursor_column: state.cursor.column,
            cursor_visible: state.cursor.visible,
        };
    }
}

/// One exact semantic boundary with a bounded viewport projection.
#[derive(Clone, Debug)]
pub struct ReviewEntry {
    /// Dense semantic ordinal, never an event ordinal.
    pub state_id: u64,
    /// Canonical source event, absent for the initial state.
    pub event_seq: Option<u64>,
    /// Exact byte offset inside the source output event.
    pub event_byte_offset: Option<u64>,
    /// Source event monotonic timestamp.
    pub monotonic_ns: u64,
    /// Full normalized semantic-state content hash.
    pub state_hash: String,
    /// Whether this boundary was eligible for presentation.
    pub presented: bool,
    /// Ghostty dirty classification, rendered as inert text.
    pub dirty: String,
    /// Exact dirty row numbers.
    pub dirty_rows: Vec<u16>,
    /// Selected viewport projection.
    pub viewport: ReviewViewport,
}

impl ReviewEntry {
    fn new(record: &StateRecord, state: &SemanticState) -> Self {
        return Self {
            state_id: record.state_id,
            event_seq: record.event_seq,
            event_byte_offset: record.event_byte_offset,
            monotonic_ns: record.monotonic_ns,
            state_hash: record.state_hash.clone(),
            presented: record.presented,
            dirty: format!("{:?}", record.ghostty_dirty),
            dirty_rows: record.ghostty_dirty_rows.clone(),
            viewport: ReviewViewport::from_state(state),
        };
    }
}

/// Contiguous exact semantic entries; no sampled-away intermediate states.
#[derive(Debug)]
pub struct ReviewPage {
    /// At most REVIEW_PAGE_STATES consecutive entries.
    pub entries: VecDeque<ReviewEntry>,
    /// Exact selected semantic ordinal.
    pub selected: u64,
}

/// A bounded page query. Long scans can always be cancelled by the caller.
#[derive(Clone, Debug)]
pub enum ReviewQuery {
    /// The final exact semantic boundary and its preceding page.
    Final,
    /// Page around an exact semantic ordinal.
    State(u64),
    /// Adjacent eligible semantic state in the requested direction.
    Move {
        /// Semantic ordinal excluded from the adjacent-state lookup.
        from: u64,
        /// Select a later ordinal when true, or an earlier ordinal when false.
        forward: bool,
        /// Restrict eligible states to presented boundaries when true.
        presented_only: bool,
    },
    /// Next/previous matching semantic viewport, wrapping at the source ends.
    Search {
        /// Start strictly beyond this ordinal; `None` starts at the directional source end.
        from: Option<u64>,
        /// Search later ordinals when true, or earlier ordinals when false, wrapping if needed.
        forward: bool,
        /// Case-sensitive literal viewport substring containing 1 through 4096 UTF-8 bytes.
        query: String,
    },
    /// First state at or after the uniquely named marker.
    Marker(String),
    /// Next/previous marker or input boundary relative to a source timestamp.
    Event {
        /// Exclusive source monotonic timestamp in nanoseconds for the event lookup.
        from_ns: u64,
        /// Select a later event when true, or an earlier event when false, without wrapping.
        forward: bool,
        /// Select marker events when true, or input-intent events when false.
        marker: bool,
    },
}

/// Authenticate a completed source and resolve a bounded exact semantic page.
///
/// No all-state history or on-disk index is constructed. Source scans and semantic
/// replay check cancellation; unchanged output bytes remain cancellable too.
pub fn load_review_page(
    session: &Path,
    query: &ReviewQuery,
    cancelled: &AtomicBool,
) -> Result<Option<ReviewPage>> {
    let source = SourceIdentity::capture(session)?;
    semantic_state_with_hash_for_session_cancellable(
        session,
        &StatePoint::LatestPresented,
        false,
        cancelled,
    )?;
    let target = match query {
        ReviewQuery::Final => None,
        ReviewQuery::State(id) => Some(*id),
        ReviewQuery::Move {
            from,
            forward,
            presented_only,
        } => {
            let mut target = None;
            visit_session_states_while(session, cancelled, |record, _| {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                if *forward {
                    if record.state_id > *from && (!presented_only || record.presented) {
                        target = Some(record.state_id);
                        return Ok(false);
                    }
                } else if record.state_id >= *from {
                    return Ok(false);
                } else if !presented_only || record.presented {
                    target = Some(record.state_id);
                }
                return Ok(true);
            })?;
            if target.is_none() {
                source.check(session)?;
                return Ok(None);
            }
            target
        }
        ReviewQuery::Search {
            from,
            forward,
            query,
        } => {
            super::validate_query(query)?;
            let mut target = None;
            let mut wrap = None;
            visit_session_states_while(session, cancelled, |record, state| {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                if !*forward
                    && from.is_some_and(|id| return record.state_id >= id)
                    && target.is_some()
                {
                    return Ok(false);
                }
                if state_text(state).contains(query) {
                    if *forward {
                        if wrap.is_none() {
                            wrap = Some(record.state_id);
                        }
                        if from.is_none_or(|id| return record.state_id > id) {
                            target = Some(record.state_id);
                            return Ok(false);
                        }
                    } else {
                        wrap = Some(record.state_id);
                        if from.is_none_or(|id| return record.state_id < id) {
                            target = Some(record.state_id);
                        }
                    }
                }
                return Ok(true);
            })?;
            let target = target.or(wrap);
            if target.is_none() {
                source.check(session)?;
                return Ok(None);
            }
            target
        }
        ReviewQuery::Marker(_) | ReviewQuery::Event { .. } => {
            let Some(time) = event_target(session, query, cancelled)? else {
                source.check(session)?;
                return Ok(None);
            };
            let mut target = None;
            visit_session_states_while(session, cancelled, |record, _| {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                target = Some(record.state_id);
                return Ok(record.monotonic_ns < time);
            })?;
            target
        }
    };
    let start = target.map(|id| return id.saturating_sub(31));
    let end = target.map(|id| return id.saturating_add(32));
    let mut entries = VecDeque::<ReviewEntry>::new();
    let mut retained_bytes = 0_usize;
    visit_session_states_while(session, cancelled, |record, state| {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if start.is_some_and(|start| return record.state_id < start) {
            return Ok(true);
        }
        if end.is_some_and(|end| return record.state_id > end) {
            return Ok(false);
        }
        let entry = ReviewEntry::new(record, state);
        let bytes = entry
            .viewport
            .text
            .len()
            .saturating_add(entry.dirty_rows.len().saturating_mul(2));
        if bytes > MAX_PAGE_TEXT_BYTES {
            return Err(review_error(
                "one semantic reviewer viewport exceeds the 16 MiB page budget",
            ));
        }
        while entries.len() >= REVIEW_PAGE_STATES
            || retained_bytes.saturating_add(bytes) > MAX_PAGE_TEXT_BYTES
        {
            if target.is_some_and(|target| {
                return entries
                    .front()
                    .is_some_and(|entry| return entry.state_id == target);
            }) {
                return Ok(false);
            }
            if let Some(evicted) = entries.pop_front() {
                retained_bytes = retained_bytes.saturating_sub(
                    evicted
                        .viewport
                        .text
                        .len()
                        .saturating_add(evicted.dirty_rows.len().saturating_mul(2)),
                );
            }
        }
        retained_bytes = retained_bytes.saturating_add(bytes);
        entries.push_back(entry);
        return Ok(true);
    })?;
    source.check(session)?;
    let selected = target.or_else(|| return entries.back().map(|entry| return entry.state_id));
    let Some(selected) = selected else {
        return Err(review_error(
            "completed session contains no semantic states",
        ));
    };
    if !entries
        .iter()
        .any(|entry| return entry.state_id == selected)
    {
        return Err(review_error(format!(
            "review state {selected} does not exist"
        )));
    }
    return Ok(Some(ReviewPage { entries, selected }));
}

fn event_target(
    session: &Path,
    query: &ReviewQuery,
    cancelled: &AtomicBool,
) -> Result<Option<u64>> {
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    let mut selected = None;
    let mut matches = 0_u64;
    while let Some(record) = cursor.next_record()? {
        if cancelled.load(Ordering::Relaxed) {
            return Err(review_error("review event query cancelled"));
        }
        match query {
            ReviewQuery::Marker(label) => {
                if matches!(&record.event, Event::Marker { label: value, .. } if value == label) {
                    matches = matches.saturating_add(1);
                    selected = Some(record.monotonic_ns);
                }
            }
            ReviewQuery::Event {
                from_ns,
                forward,
                marker,
            } => {
                let eligible = if *marker {
                    matches!(record.event, Event::Marker { .. })
                } else {
                    matches!(record.event, Event::InputIntent { .. })
                };
                if eligible
                    && if *forward {
                        record.monotonic_ns > *from_ns
                    } else {
                        record.monotonic_ns < *from_ns
                    }
                {
                    selected = Some(record.monotonic_ns);
                    if *forward {
                        break;
                    }
                }
            }
            _ => {
                return Err(review_error(
                    "review event query requires an event selector",
                ));
            }
        }
    }
    if let ReviewQuery::Marker(label) = query
        && matches != 1
    {
        return Err(review_error(format!(
            "review marker `{label}` must occur exactly once, found {matches}"
        )));
    }
    return Ok(selected);
}
