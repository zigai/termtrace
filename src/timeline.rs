//! Validated source-time ranges shared by semantic and native replay.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom},
    path::PathBuf,
    result::Result as StdResult,
};

#[cfg(test)]
use std::cell::Cell;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::Event,
    event_log::EventLog,
    ghostty_vt::{DirtyState, SemanticState},
    replay::{
        DERIVED_DIRECTORY, ReplaySeed, StateIndexEntry, StateRecord, StateSelector,
        load_nearest_replay_seed, visit_states_range, visit_states_range_while,
        visit_states_seeded_while,
    },
    vt_framer::{VtFramer, VtToken, frame_event_log},
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;

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

/// One ordered, source-bound timeline item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    /// Canonical session offset.
    pub monotonic_ns: u64,
    /// Responsible canonical event, absent for the initial semantic state.
    pub event_seq: Option<u64>,
    /// Timeline item payload.
    pub item: TimelineItem,
}

/// Concise canonical or semantic item in a diagnostic timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineItem {
    /// A distinct Ghostty semantic state transition.
    SemanticState {
        /// Dense semantic state identifier.
        state_id: u64,
        /// Byte offset within the responsible output event.
        event_byte_offset: Option<u64>,
        /// Complete normalized semantic state hash.
        state_hash: String,
        /// Whether the state was eligible for presentation.
        presented: bool,
        /// Ghostty's dirty classification.
        ghostty_dirty: DirtyState,
        /// Rows whose semantic cells changed.
        changed_rows: Vec<u16>,
        /// Number of complete semantic cell values changed.
        changed_cells: u64,
        /// Whether cursor state changed.
        cursor_changed: bool,
        /// Whether terminal modes changed.
        modes_changed: bool,
        /// Whether terminal metadata changed.
        metadata_changed: bool,
    },
    /// A non-output canonical event retained verbatim.
    CanonicalEvent {
        /// Exact typed canonical event.
        event: Event,
    },
    /// One lossless explanatory VT frame.
    VtToken {
        /// Exact token and canonical source segments.
        token: VtToken,
    },
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
        let token_start = start_key.map_or(0, |_| {
            if log
                .events
                .get(first_relevant)
                .is_some_and(|record| return matches!(record.event, Event::Output { .. }))
            {
                return first_relevant;
            }
            return log
                .events
                .get(..first_relevant)
                .unwrap_or_default()
                .iter()
                .rposition(|record| return matches!(record.event, Event::Output { .. }))
                .unwrap_or(first_relevant);
        });
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
                        event_index: token_start,
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
        let changed_rows = state
            .delta
            .rows
            .iter()
            .map(|row| return row.row)
            .collect::<Vec<_>>();
        let changed_cells = state.delta.rows.iter().fold(0_u64, |count, row| {
            return count.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
        });
        entries.push(TimelineEntry {
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
        });
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
            for token in frame_event_log(log)? {
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
    entries.sort_by_key(|entry| {
        let state_order = match &entry.item {
            TimelineItem::CanonicalEvent { .. } => 0,
            TimelineItem::VtToken { .. } => 1,
            TimelineItem::SemanticState { .. } => 2,
        };
        return (
            entry.monotonic_ns,
            entry.event_seq.unwrap_or(0),
            state_order,
            match &entry.item {
                TimelineItem::SemanticState {
                    event_byte_offset, ..
                } => event_byte_offset.unwrap_or(0),
                TimelineItem::VtToken { token } => token
                    .source
                    .first()
                    .map_or(0, |segment| return segment.start_byte_offset),
                TimelineItem::CanonicalEvent { .. } => 0,
            },
        );
    });
    return Ok(entries);
}

/// One bounded page of a concise diagnostic timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelinePage {
    /// Entries selected by the requested offset and limit.
    pub entries: Vec<TimelineEntry>,
    /// Offset for the next page, absent when this page reaches closure.
    pub next_offset: Option<u64>,
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

pub(crate) fn timeline_event(event: &Event) -> bool {
    return !matches!(
        event,
        Event::Output { .. }
            | Event::InputResult { .. }
            | Event::ResizeResult { .. }
            | Event::SignalResult { .. }
    );
}

/// A non-empty half-open interval on the canonical monotonic timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRange {
    /// Inclusive source timestamp in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive source timestamp in nanoseconds.
    pub end_monotonic_ns: u64,
}

impl SourceRange {
    /// Duration of the selected source interval.
    pub fn duration_ns(self) -> u64 {
        return self.end_monotonic_ns - self.start_monotonic_ns;
    }

    /// Whether a timestamp belongs to this half-open interval.
    pub fn contains(self, monotonic_ns: u64) -> bool {
        return monotonic_ns >= self.start_monotonic_ns && monotonic_ns < self.end_monotonic_ns;
    }

    /// Stable suffix used by range-specific no-replace artifact directories.
    pub fn artifact_suffix(self) -> String {
        return format!(
            "range-{}-{}",
            self.start_monotonic_ns, self.end_monotonic_ns
        );
    }
}

/// Resolve optional CLI bounds against a canonical event-log endpoint.
///
/// `start` is inclusive and defaults to zero. `end` is exclusive and defaults
/// to one nanosecond after the final event so that the final event is included.
/// An entirely absent request selects the complete session.
///
/// # Errors
///
/// Returns an error for reversed, empty, overflowing, or out-of-bounds ranges.
pub fn resolve_source_range(
    start: Option<u64>,
    end: Option<u64>,
    final_monotonic_ns: u64,
) -> Result<Option<SourceRange>> {
    if start.is_none() && end.is_none() {
        return Ok(None);
    }
    // A half-open range ending at the final timestamp would exclude the final
    // event. Advance by one nanosecond so an omitted end still means "through
    // the complete session" without giving ranged replay a special last-event case.
    let source_end = final_monotonic_ns.checked_add(1).ok_or_else(|| {
        return range_error("source timeline ends at u64::MAX and cannot form an exclusive bound");
    })?;
    let start = start.unwrap_or(0);
    let end = end.unwrap_or(source_end);
    if start >= end {
        return Err(range_error(format!(
            "source range must be non-empty with FROM < TO, got {start}..{end}"
        )));
    }
    if start > final_monotonic_ns {
        return Err(range_error(format!(
            "source range starts at {start} ns after the final event at {final_monotonic_ns} ns"
        )));
    }
    if end > source_end {
        return Err(range_error(format!(
            "source range ends at {end} ns after the exclusive source endpoint {source_end} ns"
        )));
    }
    return Ok(Some(SourceRange {
        start_monotonic_ns: start,
        end_monotonic_ns: end,
    }));
}

/// Resolve optional numeric or marker bounds into canonical timestamps.
///
/// Marker labels must identify exactly one canonical marker. Start markers are
/// inclusive and end markers are exclusive, matching numeric range semantics.
///
/// # Errors
///
/// Returns an error for mixed selectors, missing or duplicate marker labels,
/// or an invalid resulting source range.
pub fn resolve_source_selection(
    log: &EventLog,
    start: Option<u64>,
    end: Option<u64>,
    from_marker: Option<&str>,
    to_marker: Option<&str>,
) -> Result<Option<SourceRange>> {
    if start.is_some() && from_marker.is_some() {
        return Err(range_error(
            "choose either `--from` or `--from-marker`, not both",
        ));
    }
    if end.is_some() && to_marker.is_some() {
        return Err(range_error(
            "choose either `--to` or `--to-marker`, not both",
        ));
    }
    let start = match from_marker {
        Some(label) => Some(resolve_marker(log, label)?),
        None => start,
    };
    let end = match to_marker {
        Some(label) => Some(resolve_marker(log, label)?),
        None => end,
    };
    let final_monotonic_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    return resolve_source_range(start, end, final_monotonic_ns);
}

fn resolve_marker(log: &EventLog, label: &str) -> Result<u64> {
    if label.is_empty() {
        return Err(range_error("marker label must not be empty"));
    }
    let matches = log
        .events
        .iter()
        .filter(|record| {
            return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => return Ok(record.monotonic_ns),
        [] => {
            return Err(range_error(format!(
                "marker `{label}` does not exist in the canonical event log"
            )));
        }
        _ => {
            return Err(range_error(format!(
                "marker `{label}` occurs {} times; marker range selectors require a unique label",
                matches.len()
            )));
        }
    }
}

/// Parse a human-readable non-negative source offset into nanoseconds.
///
/// Accepted forms are a decimal number with an optional `ns`, `us`, `ms`,
/// `s`, `m`, or `h` suffix (bare numbers are seconds), plus `MM:SS.fraction`
/// and `HH:MM:SS.fraction` clock notation.
pub fn parse_time_offset_ns(value: &str) -> StdResult<u64, String> {
    if value.is_empty() {
        return Err("time offset cannot be empty".to_owned());
    }
    if value.contains(':') {
        return parse_clock_offset(value);
    }
    let (number, scale) = [
        ("ns", 1),
        ("us", 1_000),
        ("ms", 1_000_000),
        ("s", NANOS_PER_SECOND),
        ("m", NANOS_PER_MINUTE),
        ("h", NANOS_PER_HOUR),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| {
        return value
            .strip_suffix(suffix)
            .map(|number| return (number, scale));
    })
    .unwrap_or((value, NANOS_PER_SECOND));
    return parse_scaled_decimal(number, scale)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"));
}

fn parse_clock_offset(value: &str) -> StdResult<u64, String> {
    let parts = value.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_integer(minutes, "minutes")?, *seconds),
        [hours, minutes, seconds] => (
            parse_integer(hours, "hours")?,
            parse_integer(minutes, "minutes")?,
            *seconds,
        ),
        _ => {
            return Err(format!(
                "invalid time offset `{value}`: use MM:SS or HH:MM:SS"
            ));
        }
    };
    if parts.len() == 3 && minutes >= 60 {
        return Err(format!(
            "invalid time offset `{value}`: minutes must be below 60 in HH:MM:SS"
        ));
    }
    let seconds_ns = parse_scaled_decimal(seconds, NANOS_PER_SECOND)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"))?;
    if seconds_ns >= NANOS_PER_MINUTE {
        return Err(format!(
            "invalid time offset `{value}`: seconds must be below 60"
        ));
    }
    let hours_ns = hours
        .checked_mul(NANOS_PER_HOUR)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    let minutes_ns = minutes
        .checked_mul(NANOS_PER_MINUTE)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    return hours_ns
        .checked_add(minutes_ns)
        .and_then(|total| return total.checked_add(seconds_ns))
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"));
}

fn parse_scaled_decimal(number: &str, scale: u64) -> StdResult<u64, String> {
    let (whole, fraction) = number
        .split_once('.')
        .map_or((number, None), |(whole, fraction)| {
            return (whole, Some(fraction));
        });
    if whole.is_empty() || number.matches('.').count() > 1 {
        return Err("expected a non-negative decimal number".to_owned());
    }
    let whole = parse_integer(whole, "value")?;
    let whole_ns = whole
        .checked_mul(scale)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned())?;
    let Some(fraction) = fraction else {
        return Ok(whole_ns);
    };
    if fraction.is_empty() || fraction.len() > 18 {
        return Err("fraction must contain between 1 and 18 digits".to_owned());
    }
    let numerator = parse_integer(fraction, "fraction")?;
    let exponent =
        u32::try_from(fraction.len()).map_err(|_| return "fraction is too precise".to_owned())?;
    let denominator = 10_u64
        .checked_pow(exponent)
        .ok_or_else(|| return "fraction is too precise".to_owned())?;
    let scaled = u128::from(numerator) * u128::from(scale);
    if scaled % u128::from(denominator) != 0 {
        return Err("fraction is more precise than one nanosecond".to_owned());
    }
    let fraction_ns = u64::try_from(scaled / u128::from(denominator))
        .map_err(|_| return "fraction overflows nanoseconds".to_owned())?;
    return whole_ns
        .checked_add(fraction_ns)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned());
}

fn parse_integer(value: &str, component: &str) -> StdResult<u64, String> {
    return value
        .parse::<u64>()
        .map_err(|_| return format!("{component} must be a non-negative integer"));
}

fn range_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    use uuid::Uuid;

    use crate::{
        artifact::create_private_directory,
        event::{ByteString, EventLogHeader, EventRecord},
        replay::{
            StateSelection, reset_test_replay_output_bytes, test_replay_output_bytes, write_index,
        },
    };

    fn ranged_timeline_log() -> EventLog {
        let session_id = Uuid::now_v7();
        let events = vec![
            EventRecord {
                seq: 0,
                monotonic_ns: 1,
                prev_hash: "0".repeat(64),
                hash: "1".repeat(64),
                event: Event::Output {
                    data: ByteString::from(b"a".as_slice()),
                },
            },
            EventRecord {
                seq: 1,
                monotonic_ns: 2,
                prev_hash: "1".repeat(64),
                hash: "2".repeat(64),
                event: Event::Marker {
                    operation_id: Uuid::now_v7(),
                    label: "range-start".to_owned(),
                },
            },
            EventRecord {
                seq: 2,
                monotonic_ns: 3,
                prev_hash: "2".repeat(64),
                hash: "3".repeat(64),
                event: Event::Output {
                    data: ByteString::from(b"b".as_slice()),
                },
            },
        ];
        return EventLog {
            header: EventLogHeader::new(
                session_id,
                "0".repeat(64),
                0,
                80,
                24,
                800,
                480,
                "xterm-ghostty",
            ),
            events,
            pending_operations: Vec::new(),
            source_path: None,
        };
    }

    #[test]
    fn ranged_timeline_pages_from_a_replay_baseline() {
        let log = ranged_timeline_log();
        let range = Some(SourceRange {
            start_monotonic_ns: 2,
            end_monotonic_ns: 4,
        });

        let first =
            build_timeline_page(&log, false, false, range, 0, 1).expect("first timeline page");
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.next_offset, Some(1));
        let second =
            build_timeline_page(&log, false, false, range, 1, 8).expect("remaining timeline page");
        assert!(!second.entries.is_empty());
        assert_eq!(second.next_offset, None);
        assert!(
            first
                .entries
                .iter()
                .chain(&second.entries)
                .all(|entry| return (2..4).contains(&entry.monotonic_ns))
        );
    }
    fn indexed_control_log(temporary: &tempfile::TempDir) -> EventLog {
        let session_id = Uuid::now_v7();
        let mut events: Vec<EventRecord> = Vec::new();
        for seq in 0..400_u64 {
            events.push(EventRecord {
                seq,
                monotonic_ns: seq.saturating_add(1),
                prev_hash: format!("{:064x}", seq),
                hash: format!("{:064x}", seq.saturating_add(1)),
                event: Event::Output {
                    data: ByteString::from(format!("{seq:04}\x1b[H").into_bytes()),
                },
            });
        }
        let log = EventLog {
            header: EventLogHeader::new(
                session_id,
                "0".repeat(64),
                0,
                80,
                24,
                800,
                480,
                "xterm-ghostty",
            ),
            events,
            pending_operations: Vec::new(),
            source_path: Some(temporary.path().join("events.jsonl")),
        };
        let directory = temporary.path().join(DERIVED_DIRECTORY);
        create_private_directory(&directory).expect("derived directory");
        write_index(&directory, &log, None, true, StateSelection::All)
            .expect("control-rich checkpoint index");
        return log;
    }

    #[test]
    fn late_control_pages_and_ranges_seek_auxiliary_output() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let log = indexed_control_log(&temporary);
        let mut canonical = log.clone();
        canonical.source_path = None;
        let full = build_timeline(&canonical, true, true, None).expect("full control timeline");
        let offset = full.len().saturating_sub(20);
        reset_test_auxiliary_scan();
        let page = build_timeline_page(
            &log,
            true,
            true,
            None,
            u64::try_from(offset).expect("late offset"),
            5,
        )
        .expect("late control page");
        let (offset_bytes, offset_events, offset_first_event) = test_auxiliary_scan();
        assert!(
            offset_first_event > 0,
            "late page scanned controls from event zero"
        );
        assert!(
            offset_bytes < 200,
            "late page scanned {offset_bytes} control bytes"
        );
        assert!(
            offset_events < 30,
            "late page scanned {offset_events} control events"
        );
        assert_eq!(
            page.entries,
            full.iter()
                .skip(offset)
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
        );

        let range = Some(SourceRange {
            start_monotonic_ns: 350,
            end_monotonic_ns: 361,
        });
        let expected =
            build_timeline(&canonical, true, true, range).expect("full ranged control timeline");
        reset_test_auxiliary_scan();
        let ranged = build_timeline_page(&log, true, true, range, 0, 10_000)
            .expect("late ranged control page");
        let (range_bytes, range_events, range_first_event) = test_auxiliary_scan();
        assert!(
            range_first_event >= 348,
            "late range began auxiliary framing at event {range_first_event}"
        );
        assert!(
            range_bytes < 200,
            "late range scanned {range_bytes} control bytes"
        );
        assert!(
            range_events < 30,
            "late range scanned {range_events} control events"
        );
        assert_eq!(ranged.entries, expected);
        assert_eq!(ranged.next_offset, None);
    }

    #[test]
    fn indexed_range_preserves_control_spanning_output_events() {
        let template = tempfile::TempDir::new().expect("template directory");
        let mut log = indexed_control_log(&template);
        for (seq, data) in [
            (348_usize, b"\x1b]2;".as_slice()),
            (349_usize, b"spanning-title".as_slice()),
            (350_usize, b"\x07".as_slice()),
        ] {
            let Event::Output { data: output } = &mut log.events[seq].event else {
                panic!("control fixture output event");
            };
            *output = ByteString::from(data);
        }
        let temporary = tempfile::TempDir::new().expect("indexed directory");
        log.source_path = Some(temporary.path().join("events.jsonl"));
        let directory = temporary.path().join(DERIVED_DIRECTORY);
        create_private_directory(&directory).expect("derived directory");
        write_index(&directory, &log, None, true, StateSelection::All)
            .expect("spanning-control index");
        let range = Some(SourceRange {
            start_monotonic_ns: 350,
            end_monotonic_ns: 351,
        });
        let mut canonical = log.clone();
        canonical.source_path = None;
        let expected =
            build_timeline(&canonical, true, true, range).expect("canonical spanning range");
        reset_test_auxiliary_scan();
        let page = build_timeline_page(&log, true, true, range, 0, 10_000)
            .expect("indexed spanning range");
        let (bytes, events, first_event) = test_auxiliary_scan();
        assert_eq!(page.entries, expected);
        assert_eq!(page.next_offset, None);
        assert_eq!(first_event, 348);
        assert!(bytes < 100, "spanning range read {bytes} control bytes");
        assert!(events < 10, "spanning range read {events} control events");
    }
    #[test]
    fn resolves_half_open_bounds() {
        assert_eq!(
            resolve_source_range(None, None, 100).expect("full range"),
            None
        );
        assert_eq!(
            resolve_source_range(Some(20), None, 100).expect("open end"),
            Some(SourceRange {
                start_monotonic_ns: 20,
                end_monotonic_ns: 101,
            })
        );
        assert_eq!(
            resolve_source_range(None, Some(80), 100).expect("open start"),
            Some(SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 80,
            })
        );
    }

    #[test]
    fn rejects_empty_and_out_of_bounds_ranges() {
        assert!(resolve_source_range(Some(10), Some(10), 100).is_err());
        assert!(resolve_source_range(Some(101), None, 100).is_err());
        assert!(resolve_source_range(None, Some(102), 100).is_err());
    }

    #[test]
    fn parses_units_and_clock_offsets_exactly() {
        assert_eq!(parse_time_offset_ns("1.5s"), Ok(1_500_000_000));
        assert_eq!(parse_time_offset_ns("250ms"), Ok(250_000_000));
        assert_eq!(parse_time_offset_ns("2m"), Ok(120_000_000_000));
        assert_eq!(parse_time_offset_ns("01:02.25"), Ok(62_250_000_000));
        assert_eq!(parse_time_offset_ns("01:01:02.25"), Ok(3_662_250_000_000));
        assert_eq!(parse_time_offset_ns("2"), Ok(2_000_000_000));
    }

    #[test]
    fn streaming_pages_match_full_timeline_with_controls() {
        let log = ranged_timeline_log();
        let full = build_timeline(&log, true, true, None).expect("full timeline");
        for offset in 0..=full.len() {
            let page = build_timeline_page(
                &log,
                true,
                true,
                None,
                u64::try_from(offset).expect("offset"),
                2,
            )
            .expect("streamed page");
            assert_eq!(
                page.entries,
                full.iter()
                    .skip(offset)
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                page.next_offset,
                (offset.saturating_add(2) < full.len())
                    .then(|| return u64::try_from(offset.saturating_add(2)).expect("next offset"))
            );
        }
    }

    #[test]
    fn late_page_uses_checkpoint_and_retains_only_requested_entries() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let directory = temporary.path().join(DERIVED_DIRECTORY);
        create_private_directory(&directory).expect("derived directory");
        let data = vec![b'x'; 1_100];
        let mut log = EventLog {
            header: EventLogHeader::new(
                Uuid::now_v7(),
                "0".repeat(64),
                0,
                80,
                24,
                800,
                480,
                "xterm-ghostty",
            ),
            events: vec![EventRecord {
                seq: 0,
                monotonic_ns: 1,
                prev_hash: "0".repeat(64),
                hash: "1".repeat(64),
                event: Event::Output {
                    data: ByteString::from(data),
                },
            }],
            pending_operations: Vec::new(),
            source_path: Some(temporary.path().join("events.jsonl")),
        };
        write_index(&directory, &log, None, true, StateSelection::All).expect("checkpointed index");
        reset_test_replay_output_bytes();
        reset_test_max_page_entries();
        let page =
            build_timeline_page(&log, true, false, None, 1_090, 3).expect("late timeline page");
        let replayed_bytes = test_replay_output_bytes();
        assert!(
            replayed_bytes < 100,
            "late page replayed {replayed_bytes} bytes instead of a checkpoint suffix"
        );
        assert_eq!(page.entries.len(), 3);
        assert_eq!(test_max_page_entries(), 3);
        assert_eq!(page.next_offset, Some(1_093));
        log.source_path = None;
        let full = build_timeline(&log, true, false, None).expect("canonical full timeline");
        assert_eq!(
            page.entries,
            full.into_iter().skip(1_090).take(3).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rejects_invalid_offsets() {
        assert!(parse_time_offset_ns("-1s").is_err());
        assert!(parse_time_offset_ns("1:60").is_err());
        assert!(parse_time_offset_ns("0.1ns").is_err());
        assert!(parse_time_offset_ns("2m30s").is_err());
    }
}
