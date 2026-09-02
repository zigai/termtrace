use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom},
    path::PathBuf,
};

#[cfg(test)]
use std::cell::Cell;

use serde::Deserialize;

use termtrace_evidence::event_log::EventLog;
use termtrace_model::{
    execution::event::Event,
    terminal::replay::{DERIVED_DIRECTORY, StateIndexEntry, StateRecord, StateSelector},
    terminal::state::SemanticState,
    terminal::vt::VtToken,
};
use termtrace_model::{
    terminal::replay::SourceRange,
    terminal::timeline::{TimelineEntry, TimelineItem, TimelinePage},
};
use termtrace_terminal::{VtFramer, frame_event_records};

use crate::replay::{
    ReplaySeed, load_nearest_replay_seed, timeline_event, visit_states_range,
    visit_states_range_while, visit_states_seeded_while,
};
use crate::{Error, Result};

use super::selection::range_error;

#[cfg(test)]
thread_local! {
    static TEST_MAX_PAGE_ENTRIES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn reset_test_max_page_entries() {
    TEST_MAX_PAGE_ENTRIES.set(0);
}

#[cfg(test)]
fn test_max_page_entries() -> usize {
    return TEST_MAX_PAGE_ENTRIES.get();
}

#[cfg(test)]
thread_local! {
    static TEST_AUXILIARY_BYTES: Cell<u64> = const { Cell::new(0) };
    static TEST_AUXILIARY_EVENTS: Cell<u64> = const { Cell::new(0) };
    static TEST_AUXILIARY_FIRST_EVENT: Cell<u64> = const { Cell::new(u64::MAX) };
}

#[cfg(test)]
fn reset_test_auxiliary_scan() {
    TEST_AUXILIARY_BYTES.set(0);
    TEST_AUXILIARY_EVENTS.set(0);
    TEST_AUXILIARY_FIRST_EVENT.set(u64::MAX);
}

#[cfg(test)]
fn test_auxiliary_scan() -> (u64, u64, u64) {
    return (
        TEST_AUXILIARY_BYTES.get(),
        TEST_AUXILIARY_EVENTS.get(),
        TEST_AUXILIARY_FIRST_EVENT.get(),
    );
}

type TimelineKey = (u64, u64, u8, u64);

fn timeline_key(entry: &TimelineEntry) -> TimelineKey {
    let (order, byte_offset) = match &entry.item {
        TimelineItem::CanonicalEvent { .. } => (0, 0),
        TimelineItem::VtToken { token } => (
            1,
            token
                .source
                .first()
                .map_or(0, |segment| return segment.start_byte_offset),
        ),
        TimelineItem::SemanticState {
            event_byte_offset, ..
        } => (2, event_byte_offset.unwrap_or(0)),
    };
    return (
        entry.monotonic_ns,
        entry.event_seq.unwrap_or(0),
        order,
        byte_offset,
    );
}

fn semantic_timeline_entry(state: &StateRecord) -> TimelineEntry {
    let changed_rows = state
        .delta
        .rows
        .iter()
        .map(|row| return row.row)
        .collect::<Vec<_>>();
    let changed_cells = state.delta.rows.iter().fold(0_u64, |count, row| {
        return count.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
    });
    return TimelineEntry {
        monotonic_ns: state.monotonic_ns,
        event_seq: state.event_seq,
        item: TimelineItem::SemanticState {
            state_id: state.state_id,
            event_byte_offset: state.event_byte_offset,
            state_hash: state.state_hash.clone(),
            presented: state.presented,
            ghostty_dirty: state.ghostty_dirty,
            changed_rows,
            changed_cells,
            cursor_changed: state.delta.cursor.is_some(),
            modes_changed: state.delta.modes.is_some(),
            metadata_changed: state.delta.metadata.is_some(),
        },
    };
}

struct CanonicalTimelineCursor<'log> {
    log: &'log EventLog,
    range: Option<SourceRange>,
    index: usize,
}

impl<'log> CanonicalTimelineCursor<'log> {
    fn next(&mut self) -> Option<TimelineEntry> {
        while let Some(record) = self.log.events.get(self.index) {
            self.index = self.index.saturating_add(1);
            if self
                .range
                .is_some_and(|range| return !range.contains(record.monotonic_ns))
                || !timeline_event(&record.event)
            {
                continue;
            }
            return Some(TimelineEntry {
                monotonic_ns: record.monotonic_ns,
                event_seq: Some(record.seq),
                item: TimelineItem::CanonicalEvent {
                    event: record.event.clone(),
                },
            });
        }
        return None;
    }
}

struct TokenTimelineCursor<'log> {
    log: &'log EventLog,
    range: Option<SourceRange>,
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "metrics are recorded only by tests")
    )]
    record_metrics: bool,
    event_index: usize,
    byte_offset: usize,
    framer: Option<VtFramer>,
    pending: VecDeque<VtToken>,
}

impl<'log> TokenTimelineCursor<'log> {
    fn next(&mut self) -> Result<Option<TimelineEntry>> {
        loop {
            if let Some(token) = self.pending.pop_front() {
                if self.range.is_some_and(|range| {
                    return !token
                        .source
                        .iter()
                        .any(|segment| return range.contains(segment.monotonic_ns));
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
            let Some(record) = self.log.events.get(self.event_index) else {
                let Some(framer) = self.framer.take() else {
                    return Ok(None);
                };
                self.pending.extend(framer.finish()?);
                continue;
            };
            if self
                .range
                .is_some_and(|range| return record.monotonic_ns >= range.end_monotonic_ns)
            {
                self.event_index = self.log.events.len();
                let Some(framer) = self.framer.take() else {
                    return Ok(None);
                };
                self.pending.extend(framer.finish()?);
                continue;
            }
            let Event::Output { data } = &record.event else {
                self.event_index = self.event_index.saturating_add(1);
                self.byte_offset = 0;
                continue;
            };
            let Some(byte) = data.as_bytes().get(self.byte_offset).copied() else {
                self.event_index = self.event_index.saturating_add(1);
                self.byte_offset = 0;
                continue;
            };
            let offset = u64::try_from(self.byte_offset).unwrap_or(u64::MAX);
            self.byte_offset = self.byte_offset.saturating_add(1);
            let framer = self.framer.as_mut().ok_or_else(|| {
                return range_error("timeline VT cursor finished before canonical output");
            })?;
            #[cfg(test)]
            if self.record_metrics {
                TEST_AUXILIARY_BYTES.set(TEST_AUXILIARY_BYTES.get().saturating_add(1));
                if offset == 0 {
                    TEST_AUXILIARY_EVENTS.set(TEST_AUXILIARY_EVENTS.get().saturating_add(1));
                }
                TEST_AUXILIARY_FIRST_EVENT.set(TEST_AUXILIARY_FIRST_EVENT.get().min(record.seq));
            }
            self.pending.extend(framer.feed_byte_at(
                record.seq,
                record.monotonic_ns,
                offset,
                byte,
            )?);
        }
    }
}
struct IndexedTokenTimelineCursor {
    path: PathBuf,
    reader: BufReader<File>,
    line: String,
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "metrics are recorded only by tests")
    )]
    record_metrics: bool,
    range: Option<SourceRange>,
}

impl IndexedTokenTimelineCursor {
    fn open(
        log: &EventLog,
        range: Option<SourceRange>,
        offset: u64,
        record_metrics: bool,
    ) -> Result<Option<Self>> {
        let Some(source_path) = log.source_path.as_ref() else {
            return Ok(None);
        };
        let Some(session) = source_path.parent() else {
            return Ok(None);
        };
        let path = session.join(DERIVED_DIRECTORY).join("control-index.jsonl");
        if !path.is_file() {
            return Ok(None);
        }
        let mut file = File::open(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        file.seek(SeekFrom::Start(offset)).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        return Ok(Some(Self {
            path,
            reader: BufReader::new(file),
            line: String::new(),
            range,
            record_metrics,
        }));
    }

    fn next(&mut self) -> Result<Option<TimelineEntry>> {
        loop {
            self.line.clear();
            let read = self.reader.read_line(&mut self.line).map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;
            if read == 0 {
                return Ok(None);
            }
            let token: VtToken = serde_json::from_str(self.line.trim_end()).map_err(|source| {
                return Error::InvalidJson {
                    path: self.path.clone(),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            let Some(first) = token.source.first() else {
                continue;
            };
            if self
                .range
                .is_some_and(|range| return first.monotonic_ns >= range.end_monotonic_ns)
            {
                return Ok(None);
            }
            #[cfg(test)]
            if self.record_metrics {
                TEST_AUXILIARY_BYTES.set(
                    TEST_AUXILIARY_BYTES.get().saturating_add(
                        u64::try_from(token.raw.as_bytes().len()).unwrap_or(u64::MAX),
                    ),
                );
                TEST_AUXILIARY_EVENTS.set(
                    TEST_AUXILIARY_EVENTS
                        .get()
                        .saturating_add(u64::try_from(token.source.len()).unwrap_or(u64::MAX)),
                );
                TEST_AUXILIARY_FIRST_EVENT
                    .set(TEST_AUXILIARY_FIRST_EVENT.get().min(first.event_seq));
            }
            if self.range.is_some_and(|range| {
                return !token
                    .source
                    .iter()
                    .any(|segment| return range.contains(segment.monotonic_ns));
            }) {
                continue;
            }
            return Ok(Some(TimelineEntry {
                monotonic_ns: first.monotonic_ns,
                event_seq: Some(first.event_seq),
                item: TimelineItem::VtToken { token },
            }));
        }
    }
}

enum ControlTimelineCursor<'log> {
    Indexed(IndexedTokenTimelineCursor),
    Framed(TokenTimelineCursor<'log>),
}

impl ControlTimelineCursor<'_> {
    fn next(&mut self) -> Result<Option<TimelineEntry>> {
        return match self {
            Self::Indexed(cursor) => cursor.next(),
            Self::Framed(cursor) => cursor.next(),
        };
    }
}

struct AuxiliaryTimelineCursor<'log> {
    canonical: CanonicalTimelineCursor<'log>,
    tokens: Option<ControlTimelineCursor<'log>>,
    open_error: Option<Error>,
    minimum_key: Option<TimelineKey>,
    next_canonical: Option<TimelineEntry>,
    next_token: Option<TimelineEntry>,
}

impl<'log> AuxiliaryTimelineCursor<'log> {
    fn new(
        log: &'log EventLog,
        range: Option<SourceRange>,
        include_controls: bool,
        start_key: Option<TimelineKey>,
        control_index_offset: Option<u64>,
        record_metrics: bool,
    ) -> Self {
        let first_relevant = start_key
            .and_then(|key| {
                return log.events.iter().position(|record| {
                    return (record.monotonic_ns, record.seq) >= (key.0, key.1);
                });
            })
            .unwrap_or(0);
        let (tokens, open_error) = if include_controls {
            match IndexedTokenTimelineCursor::open(
                log,
                range,
                control_index_offset.unwrap_or(0),
                record_metrics,
            ) {
                Ok(Some(cursor)) => (Some(ControlTimelineCursor::Indexed(cursor)), None),
                Ok(None) => (
                    Some(ControlTimelineCursor::Framed(TokenTimelineCursor {
                        log,
                        range,
                        record_metrics,
                        event_index: 0,
                        byte_offset: 0,
                        framer: Some(VtFramer::default()),
                        pending: VecDeque::new(),
                    })),
                    None,
                ),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };
        return Self {
            canonical: CanonicalTimelineCursor {
                log,
                range,
                index: first_relevant,
            },
            tokens,
            minimum_key: range.is_none().then_some(start_key).flatten(),
            open_error,
            next_canonical: None,
            next_token: None,
        };
    }

    fn next(&mut self) -> Result<Option<TimelineEntry>> {
        if let Some(error) = self.open_error.take() {
            return Err(error);
        }
        loop {
            if self.next_canonical.is_none() {
                self.next_canonical = self.canonical.next();
            }
            if self.next_token.is_none()
                && let Some(tokens) = self.tokens.as_mut()
            {
                self.next_token = tokens.next()?;
            }
            let take_canonical = match (&self.next_canonical, &self.next_token) {
                (Some(canonical), Some(token)) => timeline_key(canonical) <= timeline_key(token),
                (Some(_), None) => true,
                (None, _) => false,
            };
            let entry = if take_canonical {
                self.next_canonical.take()
            } else {
                self.next_token.take()
            };
            let Some(entry) = entry else {
                return Ok(None);
            };
            if self
                .minimum_key
                .is_some_and(|minimum| return timeline_key(&entry) < minimum)
            {
                continue;
            }
            return Ok(Some(entry));
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlSourceIndexEntry {
    event_seq: u64,
    control_index_offset: u64,
}

fn locate_range_control_offset(log: &EventLog, range: SourceRange) -> Result<Option<u64>> {
    let Some(source_path) = log.source_path.as_ref() else {
        return Ok(None);
    };
    let Some(session) = source_path.parent() else {
        return Ok(None);
    };
    let directory = session.join(DERIVED_DIRECTORY);
    let controls_path = directory.join("control-index.jsonl");
    let source_index_path = directory.join("control-source-index.jsonl");
    if !controls_path.is_file() || !source_index_path.is_file() {
        return Ok(None);
    }
    let relevant_bounds = log
        .events
        .iter()
        .filter(|record| {
            return range.contains(record.monotonic_ns)
                && matches!(record.event, Event::Output { .. });
        })
        .fold(None, |bounds, record| {
            return Some(match bounds {
                Some((first, _)) => (first, record.seq),
                None => (record.seq, record.seq),
            });
        });
    let Some((first_event, last_event)) = relevant_bounds else {
        return fs::metadata(&controls_path)
            .map(|metadata| return Some(metadata.len()))
            .map_err(|source| {
                return Error::Filesystem {
                    path: controls_path,
                    source,
                };
            });
    };
    let file = File::open(&source_index_path).map_err(|source| {
        return Error::Filesystem {
            path: source_index_path.clone(),
            source,
        };
    })?;
    let mut offset = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|source| {
            return Error::Filesystem {
                path: source_index_path.clone(),
                source,
            };
        })?;
        let entry: ControlSourceIndexEntry = serde_json::from_str(&line).map_err(|source| {
            return Error::InvalidJson {
                path: source_index_path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        if entry.event_seq > last_event {
            break;
        }
        if entry.event_seq >= first_event {
            offset = Some(offset.map_or(entry.control_index_offset, |current: u64| {
                return current.min(entry.control_index_offset);
            }));
        }
    }
    return Ok(offset);
}

fn locate_offset_checkpoint(
    log: &EventLog,
    include_hidden_states: bool,
    include_controls: bool,
    offset: u64,
) -> Result<Option<(u64, ReplaySeed, Option<u64>)>> {
    if offset == 0 {
        return Ok(None);
    }
    let Some(event_path) = log.source_path.as_ref() else {
        return Ok(None);
    };
    let Some(session) = event_path.parent() else {
        return Ok(None);
    };
    let directory = session.join(DERIVED_DIRECTORY);
    let path = directory.join("state-index.jsonl");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Filesystem { path, source });
        }
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut hidden_before = 0_u64;
    let mut candidate = None;
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if read == 0 {
            break;
        }
        let entry: StateIndexEntry = serde_json::from_str(line.trim_end()).map_err(|source| {
            return Error::InvalidJson {
                path: path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        if entry.checkpoint
            && let (Some(mut rank), Some(control_index_offset), Some(control_rank)) = (
                entry.timeline_rank,
                entry.control_index_offset,
                entry.control_rank,
            )
        {
            if !include_hidden_states {
                rank = rank.saturating_sub(hidden_before);
            }
            if !include_controls {
                rank = rank.saturating_sub(control_rank);
            }
            if rank <= offset {
                candidate = Some((rank, entry.state_id, control_index_offset));
            } else {
                break;
            }
        }
        if !entry.presented {
            hidden_before = hidden_before.saturating_add(1);
        }
    }
    let Some((checkpoint_rank, state_id, control_index_offset)) = candidate else {
        return Ok(None);
    };
    let seed = load_nearest_replay_seed(&directory, log, &StateSelector::StateId(state_id))?
        .ok_or_else(|| return range_error("timeline checkpoint index entry has no replay seed"))?;
    return Ok(Some((checkpoint_rank, seed, Some(control_index_offset))));
}

struct TimelinePageCollector {
    offset: u64,
    limit: usize,
    seen: u64,
    entries: Vec<TimelineEntry>,
    has_next: bool,
}

impl TimelinePageCollector {
    fn push(&mut self, entry: TimelineEntry) -> bool {
        if self.seen < self.offset {
            self.seen = self.seen.saturating_add(1);
            return true;
        }
        if self.entries.len() < self.limit {
            self.entries.push(entry);
            #[cfg(test)]
            TEST_MAX_PAGE_ENTRIES.set(TEST_MAX_PAGE_ENTRIES.get().max(self.entries.len()));
            self.seen = self.seen.saturating_add(1);
            return true;
        }
        self.has_next = true;
        return false;
    }
}

/// Build a concise ordered timeline from validated canonical events.
///
/// Output byte events are represented by their resulting semantic states;
/// mutation results are omitted from the default concise view. Other events
/// retain their exact typed canonical payload.
///
/// # Errors
///
/// Returns an error when semantic replay fails.
pub fn build_timeline(
    log: &EventLog,
    include_hidden_states: bool,
    include_controls: bool,
    range: Option<SourceRange>,
) -> Result<Vec<TimelineEntry>> {
    let mut entries = Vec::new();
    visit_states_range(log, range, |state, _| {
        if !include_hidden_states && !state.presented {
            return Ok(());
        }
        entries.push(semantic_timeline_entry(state));
        return Ok(());
    })?;
    for record in &log.events {
        if range.is_some_and(|selection| return !selection.contains(record.monotonic_ns))
            || !timeline_event(&record.event)
        {
            continue;
        }
        entries.push(TimelineEntry {
            monotonic_ns: record.monotonic_ns,
            event_seq: Some(record.seq),
            item: TimelineItem::CanonicalEvent {
                event: record.event.clone(),
            },
        });
    }
    if include_controls {
        let control_index_offset = match range {
            Some(range) => locate_range_control_offset(log, range)?,
            None => None,
        };
        if let (Some(range), Some(control_index_offset)) = (range, control_index_offset) {
            let mut auxiliary = AuxiliaryTimelineCursor::new(
                log,
                Some(range),
                true,
                Some((range.start_monotonic_ns, 0, 0, 0)),
                Some(control_index_offset),
                false,
            );
            while let Some(entry) = auxiliary.next()? {
                if matches!(entry.item, TimelineItem::VtToken { .. }) {
                    entries.push(entry);
                }
            }
        } else {
            for token in frame_event_records(&log.events)? {
                let Some(first) = token.source.first() else {
                    continue;
                };
                if range.is_some_and(|range| {
                    return !token
                        .source
                        .iter()
                        .any(|segment| return range.contains(segment.monotonic_ns));
                }) {
                    continue;
                }
                entries.push(TimelineEntry {
                    monotonic_ns: first.monotonic_ns,
                    event_seq: Some(first.event_seq),
                    item: TimelineItem::VtToken { token },
                });
            }
        }
    }
    entries.sort_by_key(timeline_key);
    return Ok(entries);
}

/// Build one bounded timeline page without retaining complete semantic states.
///
/// # Errors
///
/// Returns an error when the semantic replay fails or `limit` is zero.
pub fn build_timeline_page(
    log: &EventLog,
    include_hidden_states: bool,
    include_controls: bool,
    range: Option<SourceRange>,
    offset: u64,
    limit: u16,
) -> Result<TimelinePage> {
    if limit == 0 {
        return Err(range_error("timeline page limit must be greater than zero"));
    }
    let checkpoint = if range.is_none() {
        locate_offset_checkpoint(log, include_hidden_states, include_controls, offset)?
    } else {
        None
    };
    let checkpoint_rank = checkpoint.as_ref().map_or(0, |(rank, _, _)| return *rank);
    let minimum_key = checkpoint.as_ref().map(|(_, seed, _)| {
        return timeline_key(&semantic_timeline_entry(&seed.inspection.record));
    });
    let auxiliary_start = minimum_key
        .or_else(|| return range.map(|range| return (range.start_monotonic_ns, 0, 0, 0)));
    let control_index_offset = match checkpoint
        .as_ref()
        .and_then(|(_, _, offset)| return *offset)
    {
        Some(offset) => Some(offset),
        None if include_controls => match range {
            Some(range) => locate_range_control_offset(log, range)?,
            None => None,
        },
        None => None,
    };
    let mut auxiliary = AuxiliaryTimelineCursor::new(
        log,
        range,
        include_controls,
        auxiliary_start,
        control_index_offset,
        true,
    );
    let mut pending_auxiliary = None;
    let mut collector = TimelinePageCollector {
        offset,
        limit: usize::from(limit),
        seen: checkpoint_rank,
        entries: Vec::with_capacity(usize::from(limit)),
        has_next: false,
    };
    let mut visitor = |record: &StateRecord, _: &SemanticState| {
        if !include_hidden_states && !record.presented {
            return Ok(true);
        }
        let semantic = semantic_timeline_entry(record);
        loop {
            if pending_auxiliary.is_none() {
                pending_auxiliary = auxiliary.next()?;
            }
            let Some(auxiliary_entry) = pending_auxiliary.as_ref() else {
                break;
            };
            if minimum_key.is_some_and(|minimum| return timeline_key(auxiliary_entry) < minimum) {
                pending_auxiliary.take();
                continue;
            }
            if timeline_key(auxiliary_entry) > timeline_key(&semantic) {
                break;
            }
            let entry = pending_auxiliary.take().ok_or_else(|| {
                return range_error("timeline auxiliary cursor lost its pending entry");
            })?;
            if !collector.push(entry) {
                return Ok(false);
            }
        }
        return Ok(collector.push(semantic));
    };
    match checkpoint {
        Some((_, seed, _)) => visit_states_seeded_while(log, seed, &mut visitor)?,
        None => visit_states_range_while(log, range, &mut visitor)?,
    }
    while !collector.has_next {
        let entry = match pending_auxiliary.take() {
            Some(entry) => Some(entry),
            None => auxiliary.next()?,
        };
        let Some(entry) = entry else {
            break;
        };
        if !collector.push(entry) {
            break;
        }
    }
    let next_offset = collector.has_next.then(|| {
        return offset.saturating_add(u64::try_from(collector.entries.len()).unwrap_or(u64::MAX));
    });
    return Ok(TimelinePage {
        entries: collector.entries,
        next_offset,
    });
}

#[cfg(test)]
mod tests;
