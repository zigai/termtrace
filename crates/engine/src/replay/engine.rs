use super::{between, invalid_replay, DerivedIndex, EndpointInspection};
use crate::{completion, Error, Result};
#[cfg(test)]
use std::cell::Cell;
use std::{
    borrow::Cow,
    collections::HashMap,
    iter::from_fn,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use termtrace_evidence::event_log::{EventLog, EventLogCursor};
use termtrace_evidence::session::SessionSnapshot;
use termtrace_model::{
    execution::event::{Event, EventLogHeader, EventLogPosition, EventRecord, OperationStatus},
    execution::event_log::CanonicalBoundary,
    execution::layout::SessionLayout,
    terminal::effect::TerminalEffectRecord,
    terminal::replay::{
        CollectedReplay, ReplayArtifacts, SourceRange, StateInspection, StateRecord, StateSelector,
    },
    terminal::state::{supported_geometry_cell_count, DirtyState, SemanticState},
};
use termtrace_terminal::{
    hash_semantic_row, semantic_state_hash_from_row_hashes, Format, GhosttyVt, RenderHold,
};
use uuid::Uuid;

const MAX_REPLAY_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REPLAY_CAPTURE_ATTEMPTS: u64 = 2_000_000;
const MAX_REPLAY_STATE_COUNT: u64 = 100_000;
const MAX_REPLAY_CELL_WORK: u64 = 2_000_000_000;
const MAX_REPLAY_ELAPSED: Duration = Duration::from_secs(60);
// Long output events still get a boundary state every this many positions, so checkpoints and
// seeks stay bounded inside them.
pub(super) const POSITION_STRIDE: u64 = 1 << 16;

#[cfg(test)]
thread_local! {
    static TEST_REPLAY_OUTPUT_BYTES: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_test_replay_output_bytes() {
    TEST_REPLAY_OUTPUT_BYTES.set(0);
}

#[cfg(test)]
pub(crate) fn test_replay_output_bytes() -> u64 {
    return TEST_REPLAY_OUTPUT_BYTES.get();
}

#[derive(Clone, Copy)]
pub(super) struct ReplayLimits {
    pub(super) output_bytes: u64,
    pub(super) capture_attempts: u64,
    pub(super) state_count: u64,
    pub(super) cell_work: u64,
    pub(super) elapsed: Duration,
}

pub(super) const DEFAULT_REPLAY_LIMITS: ReplayLimits = ReplayLimits {
    output_bytes: MAX_REPLAY_OUTPUT_BYTES,
    capture_attempts: MAX_REPLAY_CAPTURE_ATTEMPTS,
    state_count: MAX_REPLAY_STATE_COUNT,
    cell_work: MAX_REPLAY_CELL_WORK,
    elapsed: MAX_REPLAY_ELAPSED,
};

const INDEX_REPLAY_LIMITS: ReplayLimits = ReplayLimits {
    output_bytes: u64::MAX,
    capture_attempts: u64::MAX,
    state_count: u64::MAX,
    cell_work: u64::MAX,
    elapsed: Duration::MAX,
};

#[derive(Clone, Copy)]
struct ResizeGeometry {
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
}

/// One replay result in canonical order; an effect precedes the state its byte produced.
#[expect(
    clippy::large_enum_variant,
    reason = "each value moves straight into a visitor and is never stored, so boxing the record would only add an allocation per state"
)]
pub(crate) enum ReplayVisit<'replay> {
    State(StateRecord, &'replay SemanticState),
    Effect(TerminalEffectRecord),
}

fn states_only(
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<bool>,
) -> impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool> {
    return move |visit, engine| match visit {
        ReplayVisit::State(record, state) => return state_sink(record, state, engine),
        ReplayVisit::Effect(_) => return Ok(true),
    };
}

#[derive(Clone, Debug)]
pub(crate) struct ReplaySeed {
    pub record: StateRecord,
    pub checkpoint: Vec<u8>,
    pub source_position: Option<EventLogPosition>,
}

/// Where a replay stops: the state there is captured even when it is not a boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayStop {
    Position(u64),
    EventSeq(u64),
    MonotonicNs(u64),
}

impl ReplayStop {
    fn before(self, record: &EventRecord) -> bool {
        return match self {
            Self::Position(_) => false,
            Self::EventSeq(event_seq) => record.seq > event_seq,
            Self::MonotonicNs(monotonic_ns) => record.monotonic_ns > monotonic_ns,
        };
    }
}

#[derive(Default)]
pub(crate) struct ReplayRequest<'request> {
    pub source_range: Option<SourceRange>,
    pub seed: Option<ReplaySeed>,
    pub stop: Option<ReplayStop>,
    // Capture every position from here on, for content searches inside one span.
    pub every_from: Option<u64>,
    pub formats: &'request [Format],
    pub include_final_formats: bool,
}

pub(crate) struct ReplayEnd {
    pub artifacts: ReplayArtifacts,
    pub last: Option<(StateRecord, SemanticState)>,
    pub rendered: RenderedStateFormats,
}

// The canonical unit whose application produced the current position.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Unit {
    event_seq: Option<u64>,
    event_byte_offset: Option<u64>,
    monotonic_ns: u64,
}

struct ReplayBuilder {
    engine: GhosttyVt,
    position: u64,
    unit: Unit,
    emitted: Option<u64>,
    keep_last: bool,
    last_record: Option<StateRecord>,
    previous: Option<SemanticState>,
    previous_hash: Option<String>,
    row_hashes: Vec<blake3::Hash>,
    budget: ReplayBudget,
}

struct ReplayBudget {
    limits: ReplayLimits,
    started: Instant,
    output_bytes: u64,
    capture_attempts: u64,
    state_count: u64,
    cell_work: u64,
    current_cells: u64,
}

/// Hash a normalized semantic state with the replay state-hash contract.
///
/// Transient Ghostty dirtiness is excluded; terminal content, modes, metadata,
/// geometry, cursor, colors, and active style are included.
///
/// # Errors
///
/// Returns an error when semantic metadata or grid rows cannot be serialized.
pub fn semantic_state_hash(state: &SemanticState) -> Result<String> {
    return termtrace_terminal::semantic_state_hash(state).map_err(Error::ScreenFrame);
}

/// Replay a validated event log and collect every boundary state.
///
/// # Errors
///
/// Returns an error when libghostty-vt or state serialization fails.
pub fn replay(log: &EventLog) -> Result<CollectedReplay> {
    let mut states = Vec::new();
    let artifacts = replay_with(log, None, |record, _, _| {
        states.push(record);
        return Ok(());
    })?;

    return Ok(CollectedReplay { states, artifacts });
}

/// Visit every boundary state with its complete reconstructed value.
///
/// This streaming interface lets derived renderers consume replay without
/// retaining every complete viewport in memory.
///
/// # Errors
///
/// Returns an error when replay or visitor processing fails.
pub fn visit_states(
    log: &EventLog,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<()>,
) -> Result<()> {
    replay_log(
        log,
        DEFAULT_REPLAY_LIMITS,
        ReplayRequest::default(),
        states_only(|record, state, _| {
            visitor(&record, state)?;

            return Ok(true);
        }),
    )?;

    return Ok(());
}

/// Visit states and effects in a validated source range, using the ranged replay baseline and
/// the position identifiers shared by derived indexes.
pub(crate) fn visit_replay_range_while(
    log: &EventLog,
    source_range: Option<SourceRange>,
    visitor: impl FnMut(ReplayVisit<'_>) -> Result<bool>,
) -> Result<()> {
    let seed = match source_range {
        Some(range) => DerivedIndex::open(log)?
            .map(|index| return index.seed_before(range.start_monotonic_ns))
            .transpose()?
            .flatten(),
        None => None,
    };

    replay_log(
        log,
        DEFAULT_REPLAY_LIMITS,
        ReplayRequest {
            source_range,
            seed,
            ..ReplayRequest::default()
        },
        ignore_engine(visitor),
    )?;

    return Ok(());
}

fn ignore_engine(
    mut visitor: impl FnMut(ReplayVisit<'_>) -> Result<bool>,
) -> impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool> {
    return move |visit, _| return visitor(visit);
}

pub(crate) fn visit_replay_seeded_while(
    log: &EventLog,
    seed: ReplaySeed,
    visitor: impl FnMut(ReplayVisit<'_>) -> Result<bool>,
) -> Result<()> {
    replay_log(
        log,
        DEFAULT_REPLAY_LIMITS,
        ReplayRequest {
            seed: Some(seed),
            ..ReplayRequest::default()
        },
        ignore_engine(visitor),
    )?;

    return Ok(());
}

/// Reconstruct one state selected by position, content, event, time, or final position.
///
/// # Errors
///
/// Returns an error when the source cannot replay or no state matches.
pub fn inspect(log: &EventLog, selector: StateSelector) -> Result<StateInspection> {
    let (inspection, _) = inspect_with_formats(log, selector, &[])?;
    return Ok(inspection);
}

/// Reconstruct several independently selected states.
///
/// # Errors
///
/// Returns an error when replay fails or any selector has no matching state.
pub fn inspect_many(log: &EventLog, selectors: &[StateSelector]) -> Result<Vec<StateInspection>> {
    let index = DerivedIndex::open(log)?;
    let recording = Recording::Log(log);

    return selectors
        .iter()
        .map(|selector| {
            let (inspection, _) = select(&recording, index.as_ref(), selector.clone(), &[])?;
            return Ok(inspection);
        })
        .collect();
}

#[cfg(test)]
pub(crate) fn inspect_latest_presented_fast(log: &EventLog) -> Result<EndpointInspection> {
    return inspect_latest_presented_fast_with_limits(log, DEFAULT_REPLAY_LIMITS);
}

#[cfg(test)]
pub(super) fn inspect_latest_presented_fast_with_limits(
    log: &EventLog,
    limits: ReplayLimits,
) -> Result<EndpointInspection> {
    return inspect_latest_presented_records(
        &log.header,
        log.events
            .iter()
            .map(|record| return Ok(Cow::Borrowed(record))),
        limits,
        None,
    );
}

pub(crate) fn inspect_latest_presented_stream(
    path: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<EndpointInspection> {
    let mut cursor = EventLogCursor::open(path)?;
    let header = cursor.header().clone();
    let records = from_fn(|| return cursor.next_record().transpose())
        .map(|record| return record.map(Cow::Owned).map_err(Error::from));

    let endpoint =
        inspect_latest_presented_records(&header, records, INDEX_REPLAY_LIMITS, cancelled)?;

    let _pending = cursor.finish()?;

    return Ok(endpoint);
}

pub(crate) fn inspect_latest_presented_prefix(
    path: &Path,
    boundary: &CanonicalBoundary,
    cancelled: &AtomicBool,
) -> Result<(EventLogHeader, EndpointInspection)> {
    let mut cursor = EventLogCursor::open(path)?;
    let header = cursor.header().clone();
    let mut output_bytes = 0_u64;
    let records = from_fn(|| {
        if cursor.next_event_seq() >= boundary.next_event_seq {
            return None;
        }

        let record = cursor.next_record().transpose();

        if let Some(Ok(record)) = &record
            && let Event::Output { data } = &record.event
        {
            output_bytes = output_bytes
                .saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));
        }

        return record.map(|record| return record.map(Cow::Owned).map_err(Error::from));
    });

    let endpoint =
        inspect_latest_presented_records(&header, records, INDEX_REPLAY_LIMITS, Some(cancelled))?;

    cursor.validate_boundary(boundary.next_event_seq, &boundary.event_hash)?;

    if output_bytes != boundary.output_bytes {
        return Err(invalid_replay(
            "validated prefix output-byte count does not match its boundary",
        ));
    }
    return Ok((header, endpoint));
}

// Tracks only the latest presented state: the screen at the stop before each hold, and the
// current screen whenever no hold is open. Positions match the full boundary replay.
fn inspect_latest_presented_records<'record>(
    header: &EventLogHeader,
    records: impl Iterator<Item = Result<Cow<'record, EventRecord>>>,
    limits: ReplayLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<EndpointInspection> {
    let mut budget = ReplayBudget::new(header.cols, header.rows, limits)?;
    let mut engine = GhosttyVt::new(header.cols, header.rows, 100_000)?;
    engine.apply_terminal_colors(&header.colors)?;
    engine.set_terminfo_name(&header.term)?;
    engine.capture_render_holds()?;
    budget.charge_capture()?;

    let mut presented = engine.snapshot()?;
    let mut presented_position = 0;
    let mut presented_unit = Unit::default();
    let mut position = 0_u64;
    let mut unit = Unit::default();
    let mut resize_intents: HashMap<Uuid, ResizeGeometry> = HashMap::new();

    for record in records {
        let record = record?;

        super::trust::check_cancelled(cancelled)?;

        match &record.event {
            Event::Output { data } => {
                budget.charge_output(data.as_bytes().len())?;
                let data_bytes = data.as_bytes();

                let mut offset = 0;
                while let Some(rest) = data_bytes.get(offset..)
                    && !rest.is_empty()
                {
                    super::trust::check_cancelled(cancelled)?;

                    let unit_before = unit;
                    let step = engine.write_step(rest);
                    drop(engine.take_responses());
                    offset = offset.saturating_add(step.consumed);
                    position = advance(position, step.consumed)?;
                    unit = output_unit(&record, offset);

                    if step.hold_edge {
                        budget.charge_capture()?;
                        presented = engine.snapshot()?;
                        (presented_position, presented_unit) = (position, unit);
                    }

                    for hold in engine.take_render_holds()? {
                        if let RenderHold::Began(frame) = hold
                            && presented_position != position - 1
                        {
                            presented = *frame;
                            presented_position = position - 1;
                            presented_unit = edge_unit(&record, offset, step.consumed, unit_before);
                        }
                    }
                }
            }
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                resize_intents.insert(
                    *operation_id,
                    ResizeGeometry {
                        cols: *cols,
                        rows: *rows,
                        pixel_width: *pixel_width,
                        pixel_height: *pixel_height,
                    },
                );
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                let geometry = resize_intents.remove(operation_id).ok_or_else(|| {
                    return invalid_replay("applied resize result is missing its intent");
                })?;
                budget.set_geometry(geometry.cols, geometry.rows)?;
                engine.resize(
                    geometry.cols,
                    geometry.rows,
                    cell_size(geometry.pixel_width, geometry.cols),
                    cell_size(geometry.pixel_height, geometry.rows),
                )?;
                engine.take_render_holds()?;
                position = advance(position, 1)?;
                unit = resize_unit(&record);
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }

    if !engine.synchronized_output()? {
        budget.charge_capture()?;
        presented = engine.snapshot()?;
        (presented_position, presented_unit) = (position, unit);
    }

    let state_hash = semantic_state_hash(&presented)?;

    return Ok(EndpointInspection {
        state: presented,
        state_hash,
        state_id: presented_position,
        event_seq: presented_unit.event_seq,
        event_byte_offset: presented_unit.event_byte_offset,
        monotonic_ns: presented_unit
            .event_seq
            .map(|_| return presented_unit.monotonic_ns),
        presented: true,
    });
}

/// Format one selected state through the pinned Ghostty formatter.
///
/// # Errors
///
/// Returns an error when replay, selection, or formatter output fails.
pub fn format_state(log: &EventLog, selector: StateSelector, format: Format) -> Result<Vec<u8>> {
    let (_, rendered) = inspect_with_formats(log, selector, &[format])?;
    return rendered
        .into_iter()
        .next()
        .map(|(_, bytes)| return bytes)
        .ok_or_else(|| return invalid_replay("no semantic state matches the selector"));
}

/// Engine-native format bytes paired with their requested format.
pub type RenderedStateFormats = Vec<(Format, Vec<u8>)>;
/// One selected state and all requested engine-native representations.
pub type StateInspectionWithFormats = (StateInspection, RenderedStateFormats);

/// Resolve a batch of states from one completed session, retaining only the requested states.
///
/// # Errors
/// Rejects incomplete/changing evidence, missing selectors, and replay failures.
pub fn inspect_completed_many(
    session: &Path,
    selectors: &[StateSelector],
) -> Result<Vec<StateInspection>> {
    let validated = super::trust::ValidatedSession::open(session)?;

    if selectors.is_empty() {
        return Ok(Vec::new());
    }

    let index = completed_index(session)?;
    let recording = Recording::Session {
        path: session,
        completed: true,
        cancelled: None,
    };

    let inspections = selectors
        .iter()
        .map(|selector| {
            let (inspection, _) = select(&recording, index.as_ref(), selector.clone(), &[])?;
            return Ok(inspection);
        })
        .collect::<Result<Vec<_>>>()?;

    validated.check()?;

    return Ok(inspections);
}

/// Inspect one completed canonical session with bounded replay memory.
///
/// # Errors
/// Rejects incomplete/changing evidence, missing selectors, and replay failures.
pub fn inspect_completed_session(
    session: &Path,
    selector: StateSelector,
) -> Result<StateInspection> {
    let (inspection, _) = inspect_completed_session_with_formats(session, selector, &[])?;
    return Ok(inspection);
}

/// Inspect and format one completed state in a single streamed replay.
///
/// # Errors
/// Rejects incomplete/changing evidence, missing selectors, and replay/format failures.
pub fn inspect_completed_session_with_formats(
    session: &Path,
    selector: StateSelector,
    formats: &[Format],
) -> Result<StateInspectionWithFormats> {
    let validated = super::trust::ValidatedSession::open(session)?;
    let inspection = inspect_session_stream_with_formats(session, selector, formats)?;

    validated.check()?;

    return Ok(inspection);
}

/// Reconstruct one selected state and requested engine-native formats.
///
/// # Errors
///
/// Returns an error when replay, selection, or formatting fails.
pub fn inspect_with_formats(
    log: &EventLog,
    selector: StateSelector,
    formats: &[Format],
) -> Result<StateInspectionWithFormats> {
    let index = DerivedIndex::open(log)?;

    return select(&Recording::Log(log), index.as_ref(), selector, formats);
}

fn completed_index(session: &Path) -> Result<Option<DerivedIndex>> {
    let Some(completion) = completion::published(session)? else {
        return Ok(None);
    };
    return DerivedIndex::open_completed(session, &completion);
}

enum Recording<'source> {
    Log(&'source EventLog),
    Session {
        path: &'source Path,
        completed: bool,
        cancelled: Option<&'source AtomicBool>,
    },
}

impl Recording<'_> {
    fn replay(
        &self,
        request: ReplayRequest<'_>,
        limits: ReplayLimits,
        visitor: impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<ReplayEnd> {
        return match self {
            Self::Log(log) => replay_log(log, limits, request, visitor),
            Self::Session {
                path,
                completed,
                cancelled,
            } => {
                let never_cancelled = AtomicBool::new(false);

                replay_session(
                    path,
                    cancelled.unwrap_or(&never_cancelled),
                    *completed,
                    request,
                    visitor,
                )
            }
        };
    }

    fn check_cancelled(&self) -> Result<()> {
        if let Self::Session { cancelled, .. } = self {
            super::trust::check_cancelled(*cancelled)?;
        }

        return Ok(());
    }
}

fn no_match() -> Error {
    return invalid_replay("no semantic state matches the selector");
}

// Every selector names one position. Replay stops there, so the last emitted state is the
// selected one, captured even when the position is not a boundary.
fn select(
    recording: &Recording<'_>,
    index: Option<&DerivedIndex>,
    selector: StateSelector,
    formats: &[Format],
) -> Result<StateInspectionWithFormats> {
    let stop = match selector {
        StateSelector::StateId(state_id) => Some(ReplayStop::Position(state_id)),
        StateSelector::StateHash(state_hash) => Some(ReplayStop::Position(
            position_with_hash(recording, index, &state_hash)?.ok_or_else(no_match)?,
        )),
        StateSelector::EventSeq(event_seq) => Some(ReplayStop::EventSeq(event_seq)),
        StateSelector::MonotonicNs(monotonic_ns) => Some(ReplayStop::MonotonicNs(monotonic_ns)),
        StateSelector::LatestPresented => {
            Some(ReplayStop::Position(latest_presented(recording, index)?))
        }
        StateSelector::Final => None,
    };

    let seed = index
        .map(|index| return index.seed_for(stop))
        .transpose()?
        .flatten();

    let end = recording.replay(
        ReplayRequest {
            seed,
            stop,
            formats,
            ..ReplayRequest::default()
        },
        DEFAULT_REPLAY_LIMITS,
        |_, _| return Ok(true),
    )?;
    recording.check_cancelled()?;

    let (record, state) = end.last.ok_or_else(no_match)?;

    if let Some(ReplayStop::Position(state_id)) = stop
        && record.state_id != state_id
    {
        return Err(no_match());
    }
    return Ok((StateInspection { record, state }, end.rendered));
}

fn latest_presented(recording: &Recording<'_>, index: Option<&DerivedIndex>) -> Result<u64> {
    if let Some(index) = index {
        return index.latest_presented_state_id().ok_or_else(no_match);
    }

    let mut latest = None;
    recording.replay(
        ReplayRequest::default(),
        DEFAULT_REPLAY_LIMITS,
        states_only(|record, _, _| {
            if record.presented {
                latest = Some(record.state_id);
            }

            return Ok(true);
        }),
    )?;

    return latest.ok_or_else(no_match);
}

// A hash names the latest position with that content among the runs a boundary state shows. The
// listed boundaries locate the run, and replaying every position up to the next boundary finds
// where it ends. Content that never reaches a boundary costs a replay of every position.
fn position_with_hash(
    recording: &Recording<'_>,
    index: Option<&DerivedIndex>,
    state_hash: &str,
) -> Result<Option<u64>> {
    let (boundaries, final_state_id) = match index.filter(|index| return index.lists_every_state())
    {
        Some(index) => (index.listed_states()?, index.final_state_id()),
        None => {
            let mut boundaries = Vec::new();
            let end = recording.replay(
                ReplayRequest::default(),
                DEFAULT_REPLAY_LIMITS,
                states_only(|record, _, _| {
                    boundaries.push((record.state_id, record.state_hash));

                    return Ok(true);
                }),
            )?;

            (boundaries, end.artifacts.final_state_id)
        }
    };

    let Some(last) = boundaries
        .iter()
        .rposition(|(_, hash)| return hash == state_hash)
    else {
        return last_position_with_hash(recording, index, state_hash, None, final_state_id);
    };
    let mut from_last = boundaries.iter().skip(last);
    let Some(&(from, _)) = from_last.next() else {
        return Ok(None);
    };
    let to = from_last
        .next()
        .map_or(final_state_id, |(next, _)| return next.saturating_sub(1));

    if to <= from {
        return Ok(Some(from));
    }
    return last_position_with_hash(recording, index, state_hash, Some(from), to);
}

// With `from`, the end of the run of `state_hash` that starts there; without it, the latest
// position in `0..=to` with that content.
fn last_position_with_hash(
    recording: &Recording<'_>,
    index: Option<&DerivedIndex>,
    state_hash: &str,
    from: Option<u64>,
    to: u64,
) -> Result<Option<u64>> {
    let first = from.map_or(0, |position| return position.saturating_add(1));
    let seed = index
        .map(|index| return index.seed_for(Some(ReplayStop::Position(first))))
        .transpose()?
        .flatten();

    let mut found = from;

    recording.replay(
        ReplayRequest {
            seed,
            stop: Some(ReplayStop::Position(to)),
            every_from: Some(first),
            ..ReplayRequest::default()
        },
        INDEX_REPLAY_LIMITS,
        states_only(|record, _, _| {
            if record.state_id < first {
                return Ok(true);
            }

            if record.state_hash == state_hash {
                found = Some(record.state_id);
            } else if from.is_some() {
                return Ok(false);
            }

            return Ok(true);
        }),
    )?;

    return Ok(found);
}

pub(crate) fn inspect_session_stream(
    session: &Path,
    selector: StateSelector,
    cancelled: Option<&AtomicBool>,
) -> Result<StateInspection> {
    return inspect_recording_stream(session, selector, cancelled, true);
}

pub(crate) fn inspect_prefix_stream(
    session: &Path,
    selector: StateSelector,
) -> Result<StateInspection> {
    return inspect_recording_stream(session, selector, None, false);
}

fn inspect_recording_stream(
    session: &Path,
    selector: StateSelector,
    cancelled: Option<&AtomicBool>,
    completed: bool,
) -> Result<StateInspection> {
    let index = if completed { completed_index(session)? } else { None };
    let recording = Recording::Session {
        path: session,
        completed,
        cancelled,
    };
    let (inspection, _) = select(&recording, index.as_ref(), selector, &[])?;
    return Ok(inspection);
}

pub(crate) fn inspect_session_stream_with_formats(
    session: &Path,
    selector: StateSelector,
    formats: &[Format],
) -> Result<StateInspectionWithFormats> {
    let index = completed_index(session)?;
    let recording = Recording::Session {
        path: session,
        completed: true,
        cancelled: None,
    };

    return select(&recording, index.as_ref(), selector, formats);
}

pub(super) fn replay_with(
    log: &EventLog,
    source_range: Option<SourceRange>,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits(log, source_range, DEFAULT_REPLAY_LIMITS, state_sink);
}

pub(super) fn replay_for_index<'record>(
    header: &EventLogHeader,
    records: impl Iterator<Item = Result<Cow<'record, EventRecord>>>,
    source_range: Option<SourceRange>,
    mut sink: impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let end = replay_records(
        ReplayBuilder::new(header, INDEX_REPLAY_LIMITS)?,
        records,
        ReplayPlan {
            source_range,
            include_final_formats: true,
            ..ReplayPlan::default()
        },
        |visit, engine| {
            sink(visit, engine)?;

            return Ok(true);
        },
    )?;
    return Ok(end.artifacts);
}

pub(super) fn replay_with_limits(
    log: &EventLog,
    source_range: Option<SourceRange>,
    limits: ReplayLimits,
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let end = replay_log(
        log,
        limits,
        ReplayRequest {
            source_range,
            include_final_formats: true,
            ..ReplayRequest::default()
        },
        states_only(|record, state, engine| {
            state_sink(record, state, engine)?;

            return Ok(true);
        }),
    )?;
    return Ok(end.artifacts);
}

#[cfg(test)]
pub(super) fn replay_with_seed_and_limits(
    log: &EventLog,
    source_range: Option<SourceRange>,
    seed: ReplaySeed,
    limits: ReplayLimits,
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let end = replay_log(
        log,
        limits,
        ReplayRequest {
            source_range,
            seed: Some(seed),
            ..ReplayRequest::default()
        },
        states_only(|record, state, engine| {
            state_sink(record, state, engine)?;

            return Ok(true);
        }),
    )?;
    return Ok(end.artifacts);
}

#[cfg(test)]
pub(super) fn validate_replay_seed(log: &EventLog, seed: &ReplaySeed) -> Result<()> {
    drop(ReplayBuilder::from_seed(log, DEFAULT_REPLAY_LIMITS, seed)?);

    return Ok(());
}

pub(super) fn replay_log(
    log: &EventLog,
    limits: ReplayLimits,
    request: ReplayRequest<'_>,
    sink: impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
) -> Result<ReplayEnd> {
    let builder = match request.seed.as_ref() {
        Some(seed) => ReplayBuilder::from_seed(log, limits, seed)?,
        None => ReplayBuilder::new(&log.header, limits)?,
    };

    return replay_records(
        builder,
        log.events
            .iter()
            .map(|record| return Ok(Cow::Borrowed(record))),
        ReplayPlan::of(&request, None),
        sink,
    );
}

/// Visit boundary states without retaining the log or state history.
///
/// The caller must establish completed-source integrity before using partial results.
/// Returning `false` stops immediately; this supports bounded pages and cancellation.
///
/// # Errors
///
/// Rejects invalid visited records, concurrent source changes, and replay resource exhaustion.
pub fn visit_session_states_while(
    session: &Path,
    cancelled: &AtomicBool,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<bool>,
) -> Result<()> {
    replay_session(
        session,
        cancelled,
        true,
        ReplayRequest::default(),
        states_only(|record, state, _| return visitor(&record, state)),
    )?;

    return Ok(());
}

/// Visit boundary states in a half-open range without retaining history.
///
/// The caller must validate completed-source integrity before using partial results.
///
/// # Errors
/// Rejects invalid visited records, source changes, and replay failures.
pub fn visit_session_states_range_while(
    session: &Path,
    range: Option<SourceRange>,
    cancelled: &AtomicBool,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<bool>,
) -> Result<()> {
    replay_session(
        session,
        cancelled,
        true,
        ReplayRequest {
            source_range: range,
            ..ReplayRequest::default()
        },
        states_only(|record, state, _| return visitor(&record, state)),
    )?;

    return Ok(());
}

/// Visit boundary states in an owned, verified finite prefix without claiming completion.
///
/// # Errors
/// Rejects modified snapshots, replay failures, and cancellation.
pub fn visit_snapshot_states_while(
    snapshot: &SessionSnapshot,
    range: Option<SourceRange>,
    cancelled: &AtomicBool,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<bool>,
) -> Result<()> {
    replay_session(
        snapshot.path(),
        cancelled,
        false,
        ReplayRequest {
            source_range: range,
            ..ReplayRequest::default()
        },
        states_only(|record, state, _| return visitor(&record, state)),
    )?;

    return Ok(());
}

pub(crate) fn visit_recording_replay_range_while(
    session: &Path,
    completed: bool,
    range: Option<SourceRange>,
    cancelled: &AtomicBool,
    visitor: impl FnMut(ReplayVisit<'_>) -> Result<bool>,
) -> Result<()> {
    replay_session(
        session,
        cancelled,
        completed,
        ReplayRequest {
            source_range: range,
            ..ReplayRequest::default()
        },
        ignore_engine(visitor),
    )?;

    return Ok(());
}

fn replay_session(
    session: &Path,
    cancelled: &AtomicBool,
    completed: bool,
    request: ReplayRequest<'_>,
    visitor: impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
) -> Result<ReplayEnd> {
    let source = if completed {
        super::trust::SourceIdentity::capture(session)?
    } else {
        super::trust::SourceIdentity::capture_candidate(session)?
    };

    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let seed = request.seed.as_ref();

    // Only a validated session vouches for the prefix that resuming skips.
    let resumed = match seed.and_then(|seed| return seed.source_position.as_ref()) {
        Some(position) if completed => {
            let validated = super::trust::ValidatedSession::open(session)?;

            cursor.resume_at(position)?;

            Some(validated)
        }
        _ => None,
    };

    let builder = match seed {
        Some(seed) => ReplayBuilder::from_seed_state(cursor.header(), INDEX_REPLAY_LIMITS, seed)?,
        None => ReplayBuilder::new(cursor.header(), INDEX_REPLAY_LIMITS)?,
    };

    let seed_source = seed.and_then(|seed| {
        let record = &seed.record;
        return record
            .event_seq
            .map(|seq| return (seq, record.monotonic_ns));
    });

    let mut seed_source_seen = false;
    let mut exhausted = false;
    let records = from_fn(|| {
        let next = cursor.next_record().transpose();
        exhausted = next.is_none();
        return next;
    })
    .map(|record| {
        let record = record.map_err(Error::from)?;

        if let Some((seq, monotonic_ns)) = seed_source
            && record.seq == seq
        {
            if record.monotonic_ns != monotonic_ns {
                return Err(invalid_replay(
                    "replay checkpoint timestamp does not match its source event",
                ));
            }

            seed_source_seen = true;
        }

        return Ok(Cow::Owned(record));
    });

    let end = replay_records(
        builder,
        records,
        ReplayPlan::of(&request, Some(cancelled)),
        visitor,
    )?;

    if exhausted && seed_source.is_some() && !seed_source_seen {
        return Err(invalid_replay(
            "replay checkpoint source event is not in the event log",
        ));
    }

    if exhausted && completed {
        if let Some(validated) = &resumed {
            let completion = validated.completion();
            cursor.validate_boundary(completion.event_count, &completion.final_event_hash)?;
        } else {
            let _pending = cursor.finish()?;
        }
    }

    if completed {
        source.check(session)?;
    } else if source != super::trust::SourceIdentity::capture_candidate(session)? {
        return Err(invalid_replay("canonical prefix changed during replay"));
    }

    return Ok(end);
}

#[derive(Default)]
struct ReplayPlan<'plan> {
    source_range: Option<SourceRange>,
    include_final_formats: bool,
    formats: &'plan [Format],
    seed: Option<&'plan StateRecord>,
    stop: Option<ReplayStop>,
    every_from: Option<u64>,
    keep_last: bool,
    cancelled: Option<&'plan AtomicBool>,
}

impl<'plan> ReplayPlan<'plan> {
    fn of(request: &'plan ReplayRequest<'plan>, cancelled: Option<&'plan AtomicBool>) -> Self {
        return Self {
            source_range: request.source_range,
            include_final_formats: request.include_final_formats,
            formats: request.formats,
            seed: request.seed.as_ref().map(|seed| return &seed.record),
            stop: request.stop,
            every_from: request.every_from,
            keep_last: true,
            cancelled,
        };
    }

    fn cancelled(&self) -> bool {
        return self
            .cancelled
            .is_some_and(|flag| return flag.load(Ordering::Relaxed));
    }

    fn stop_position(&self) -> Option<u64> {
        return match self.stop {
            Some(ReplayStop::Position(position)) => Some(position),
            _ => None,
        };
    }
}

fn replay_records<'record>(
    mut builder: ReplayBuilder,
    records: impl Iterator<Item = Result<Cow<'record, EventRecord>>>,
    plan: ReplayPlan<'_>,
    mut sink: impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
) -> Result<ReplayEnd> {
    let source_range = plan.source_range;
    let mut resize_intents = HashMap::new();
    let mut running = true;
    builder.keep_last = plan.keep_last;

    if source_range.is_none() {
        running = match plan.seed {
            Some(seed) => builder.emit_seed(seed, &mut sink)?,
            None => builder.capture(&mut sink)?,
        } && !builder.reached(&plan);
    }

    let mut emitted_range_baseline = false;

    if let (Some(range), Some(record)) = (source_range, plan.seed)
        && record.monotonic_ns >= range.start_monotonic_ns
    {
        return Err(invalid_replay(
            "replay checkpoint is not earlier than the requested source range",
        ));
    }

    'events: for record in records {
        let record = record?;

        if !running || plan.cancelled() {
            break;
        }

        let Some(start) = source_byte_start(&record, plan.seed, &mut resize_intents)? else {
            continue;
        };

        if source_range.is_some_and(|range| return record.monotonic_ns >= range.end_monotonic_ns) {
            break;
        }

        if plan.stop.is_some_and(|stop| return stop.before(&record)) {
            builder.capture(&mut sink)?;
            running = false;

            break;
        }

        let emit = source_range.is_none_or(|range| return range.contains(record.monotonic_ns));

        if !emitted_range_baseline
            && let Some(range) = source_range
            && record.monotonic_ns >= range.start_monotonic_ns
        {
            // Capture before applying an event exactly at `start`. This preserves
            // the [start, end) contract: the baseline is the boundary state, and the
            // event at the inclusive boundary remains an observable transition.
            running = builder.capture_baseline(range.start_monotonic_ns, &mut sink)?;
            emitted_range_baseline = true;

            if !running {
                break;
            }
        }

        match &record.event {
            Event::Output { data } => {
                let bytes = data.as_bytes().get(start..).ok_or_else(|| {
                    return invalid_replay("replay checkpoint output cursor is outside its event");
                })?;
                builder.budget.charge_output(bytes.len())?;

                if !emit {
                    if !builder.feed_quietly(&record, bytes, start, &plan)? {
                        break 'events;
                    }

                    continue;
                }

                running = builder.feed(&record, bytes, start, &plan, &mut sink)?
                    && builder.capture(&mut sink)?
                    && !builder.reached(&plan);
            }
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                resize_intents.insert(
                    *operation_id,
                    ResizeGeometry {
                        cols: *cols,
                        rows: *rows,
                        pixel_width: *pixel_width,
                        pixel_height: *pixel_height,
                    },
                );
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                let geometry = resize_intents.remove(operation_id).ok_or_else(|| {
                    return invalid_replay("resize result without corresponding intent");
                })?;
                builder.engine.resize(
                    geometry.cols,
                    geometry.rows,
                    cell_size(geometry.pixel_width, geometry.cols),
                    cell_size(geometry.pixel_height, geometry.rows),
                )?;
                drop(builder.engine.take_responses());
                drop(builder.engine.take_effects());
                builder.engine.take_render_holds()?;

                builder.budget.set_geometry(geometry.cols, geometry.rows)?;
                builder.position = advance(builder.position, 1)?;
                builder.unit = resize_unit(&record);

                if emit {
                    running = builder.capture(&mut sink)? && !builder.reached(&plan);
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }

    if running
        && let Some(range) = source_range
        && !emitted_range_baseline
    {
        builder.capture_baseline(range.start_monotonic_ns, &mut sink)?;
    }

    let final_state_hash = builder
        .previous_hash
        .clone()
        .ok_or_else(|| return invalid_replay("replay produced no initial state"))?;

    let (final_text, final_vt, final_html) = if plan.include_final_formats {
        (
            builder.engine.format(Format::Plain)?,
            builder.engine.format(Format::Vt)?,
            builder.engine.format(Format::Html)?,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    let rendered = plan
        .formats
        .iter()
        .map(|format| return Ok((*format, builder.engine.format(*format)?)))
        .collect::<Result<RenderedStateFormats>>()?;

    return Ok(ReplayEnd {
        artifacts: ReplayArtifacts {
            final_state_id: builder.emitted.unwrap_or(builder.position),
            final_state_hash,
            final_text,
            final_vt,
            final_html,
        },
        last: builder.last_record.take().zip(builder.previous.take()),
        rendered,
    });
}

fn source_byte_start(
    record: &EventRecord,
    seed: Option<&StateRecord>,
    resize_intents: &mut HashMap<Uuid, ResizeGeometry>,
) -> Result<Option<usize>> {
    let Some(seed) = seed else {
        return Ok(Some(0));
    };
    let Some(seed_event_seq) = seed.event_seq else {
        return Ok(Some(0));
    };
    if record.seq > seed_event_seq {
        return Ok(Some(0));
    }

    if record.seq < seed_event_seq {
        track_resize_control(record, resize_intents);

        return Ok(None);
    }

    let Some(offset) = seed.event_byte_offset else {
        track_resize_control(record, resize_intents);

        return Ok(None);
    };

    if !matches!(record.event, Event::Output { .. }) {
        return Err(invalid_replay(
            "replay checkpoint byte cursor does not identify an output event",
        ));
    }

    let start = offset
        .checked_add(1)
        .and_then(|value| return usize::try_from(value).ok())
        .ok_or_else(|| return invalid_replay("replay checkpoint byte cursor overflow"))?;

    return Ok(Some(start));
}

fn track_resize_control(record: &EventRecord, resize_intents: &mut HashMap<Uuid, ResizeGeometry>) {
    match &record.event {
        Event::ResizeIntent {
            operation_id,
            cols,
            rows,
            pixel_width,
            pixel_height,
        } => {
            resize_intents.insert(
                *operation_id,
                ResizeGeometry {
                    cols: *cols,
                    rows: *rows,
                    pixel_width: *pixel_width,
                    pixel_height: *pixel_height,
                },
            );
        }
        Event::ResizeResult { operation_id, .. } => {
            resize_intents.remove(operation_id);
        }
        _ => {}
    }
}

fn advance(position: u64, units: usize) -> Result<u64> {
    return u64::try_from(units)
        .ok()
        .and_then(|units| return position.checked_add(units))
        .ok_or_else(|| return invalid_replay("stream position overflow"));
}

// `end` counts the event's bytes applied so far, so the last applied byte is `end - 1`.
fn output_unit(record: &EventRecord, end: usize) -> Unit {
    return Unit {
        event_seq: Some(record.seq),
        event_byte_offset: end
            .checked_sub(1)
            .and_then(|offset| return u64::try_from(offset).ok()),
        monotonic_ns: record.monotonic_ns,
    };
}

fn resize_unit(record: &EventRecord) -> Unit {
    return Unit {
        event_seq: Some(record.seq),
        event_byte_offset: None,
        monotonic_ns: record.monotonic_ns,
    };
}

// The unit before a hold's final byte, which ended a step of `consumed` bytes at `end`.
fn edge_unit(record: &EventRecord, end: usize, consumed: usize, before_step: Unit) -> Unit {
    if consumed >= 2 {
        return output_unit(record, end.saturating_sub(1));
    }
    return before_step;
}

impl ReplayBuilder {
    fn new(header: &EventLogHeader, limits: ReplayLimits) -> Result<Self> {
        let mut engine = GhosttyVt::new(header.cols, header.rows, 100_000)?;
        engine.apply_terminal_colors(&header.colors)?;
        engine.set_terminfo_name(&header.term)?;
        engine.capture_effects()?;
        engine.capture_render_holds()?;

        return Ok(Self {
            engine,
            position: 0,
            unit: Unit::default(),
            emitted: None,
            keep_last: false,
            last_record: None,
            previous: None,
            previous_hash: None,
            row_hashes: Vec::new(),
            budget: ReplayBudget::new(header.cols, header.rows, limits)?,
        });
    }

    fn from_seed(log: &EventLog, limits: ReplayLimits, seed: &ReplaySeed) -> Result<Self> {
        let record = &seed.record;
        if let Some(event_seq) = record.event_seq {
            let source = log
                .events
                .iter()
                .find(|candidate| return candidate.seq == event_seq)
                .ok_or_else(|| {
                    return invalid_replay(
                        "replay checkpoint source event is not in the event log",
                    );
                })?;

            if source.monotonic_ns != record.monotonic_ns {
                return Err(invalid_replay(
                    "replay checkpoint timestamp does not match its source event",
                ));
            }

            if let Some(offset) = record.event_byte_offset {
                let Event::Output { data } = &source.event else {
                    return Err(invalid_replay(
                        "replay checkpoint byte cursor does not identify output",
                    ));
                };
                if offset >= u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX) {
                    return Err(invalid_replay(
                        "replay checkpoint byte cursor is outside its source event",
                    ));
                }
            }
        }

        return Self::from_seed_state(&log.header, limits, seed);
    }

    fn from_seed_state(
        header: &EventLogHeader,
        limits: ReplayLimits,
        seed: &ReplaySeed,
    ) -> Result<Self> {
        let record = &seed.record;
        let mut engine = GhosttyVt::restore_checkpoint(&seed.checkpoint, 100_000)?;
        engine.set_terminfo_name(&header.term)?;
        let mut restored = engine.snapshot()?;
        restored.dirty = record.ghostty_dirty;
        restored.dirty_rows.clone_from(&record.ghostty_dirty_rows);

        for row in &mut restored.grid {
            row.dirty = record.ghostty_dirty_rows.contains(&row.row);
        }

        let mut builder = Self {
            engine,
            position: record.state_id,
            unit: Unit {
                event_seq: record.event_seq,
                event_byte_offset: record.event_byte_offset,
                monotonic_ns: record.monotonic_ns,
            },
            emitted: Some(record.state_id),
            keep_last: false,
            last_record: None,
            previous: None,
            previous_hash: None,
            row_hashes: Vec::new(),
            budget: ReplayBudget::new(restored.cols, restored.rows, limits)?,
        };
        let restored_hash = builder.hash_state(&restored)?;

        // Restoring is exact, so a mismatch means the checkpoint and its catalog record disagree.
        if restored_hash != record.state_hash {
            return Err(invalid_replay(
                "replay checkpoint does not restore its recorded semantic state",
            ));
        }

        builder.previous = Some(restored);
        builder.previous_hash = Some(restored_hash);
        return Ok(builder);
    }

    fn reached(&self, plan: &ReplayPlan<'_>) -> bool {
        return plan.stop_position() == Some(self.position);
    }

    // The next position that must be captured even when no boundary falls there.
    fn next_capture(&self, plan: &ReplayPlan<'_>) -> u64 {
        let stride = (self.position / POSITION_STRIDE)
            .saturating_add(1)
            .saturating_mul(POSITION_STRIDE);

        let next = self.position.saturating_add(1);
        let every = plan
            .every_from
            .filter(|first| return next >= *first)
            .map(|_| return next);

        return [Some(stride), plan.stop_position(), every]
            .into_iter()
            .flatten()
            .filter(|position| return *position > self.position)
            .min()
            .unwrap_or(stride);
    }

    fn feed_quietly(
        &mut self,
        source: &EventRecord,
        bytes: &[u8],
        first_offset: usize,
        plan: &ReplayPlan<'_>,
    ) -> Result<bool> {
        let mut consumed = 0;

        for chunk in bytes.chunks(4_096) {
            if plan.cancelled() {
                return Ok(false);
            }

            self.budget.check_elapsed()?;

            let mut rest = chunk;

            while !rest.is_empty() {
                let step = self.engine.write_step(rest);
                rest = rest.get(step.consumed..).unwrap_or_default();
            }

            drop(self.engine.take_responses());
            drop(self.engine.take_effects());
            self.engine.take_render_holds()?;
            consumed += chunk.len();
        }

        self.position = advance(self.position, consumed)?;

        if consumed > 0 {
            self.unit = output_unit(source, first_offset.saturating_add(consumed));
        }

        return Ok(true);
    }

    // Bytes go to Ghostty in bulk; states are captured only at boundaries: after effects, at the
    // stops before a hold begins and ends, after its end, and at forced positions.
    fn feed(
        &mut self,
        source: &EventRecord,
        bytes: &[u8],
        first_offset: usize,
        plan: &ReplayPlan<'_>,
        sink: &mut impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let mut consumed = 0;
        let mut next_check = 0;
        while let Some(rest) = bytes.get(consumed..)
            && !rest.is_empty()
        {
            if consumed >= next_check {
                if plan.cancelled() {
                    return Ok(false);
                }

                self.budget.check_elapsed()?;
                next_check = consumed.saturating_add(4_096);
            }

            let forced = self.next_capture(plan);
            let limit = usize::try_from(forced - self.position).unwrap_or(usize::MAX);
            let chunk = rest.get(..limit.min(rest.len())).unwrap_or_default();
            let before_step = self.unit;
            let step = self.engine.write_step(chunk);
            drop(self.engine.take_responses());
            consumed = consumed.saturating_add(step.consumed);
            self.position = advance(self.position, step.consumed)?;
            let end = first_offset.saturating_add(consumed);
            self.unit = output_unit(source, end);

            let mut boundary = step.hold_edge || step.release_edge || self.position == forced;
            for hold in self.engine.take_render_holds()? {
                match hold {
                    // Ghostty saw a hold the step did not stop before; its frame is the best
                    // record of the position just before the final byte.
                    RenderHold::Began(frame) => {
                        let edge = self.position.saturating_sub(1);

                        if self.emitted != Some(edge) {
                            let unit = edge_unit(source, end, step.consumed, before_step);

                            if !self.emit_state(*frame, edge, unit, sink)? {
                                return Ok(false);
                            }
                        }
                    }
                    RenderHold::Ended => boundary = true,
                }
            }

            let effects = self.engine.take_effects();
            boundary |= !effects.is_empty();

            for effect in effects {
                let record = TerminalEffectRecord {
                    event_seq: source.seq,
                    event_byte_offset: self.unit.event_byte_offset.unwrap_or_default(),
                    monotonic_ns: source.monotonic_ns,
                    effect,
                };

                if !sink(ReplayVisit::Effect(record), &self.engine)? {
                    return Ok(false);
                }
            }

            if boundary && !self.capture(sink)? {
                return Ok(false);
            }

            if self.reached(plan) {
                return Ok(false);
            }
        }

        return Ok(true);
    }

    fn emit_seed(
        &mut self,
        seed: &StateRecord,
        sink: &mut impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let state = self
            .previous
            .as_ref()
            .ok_or_else(|| return invalid_replay("replay checkpoint restored no state"))?;

        if self.keep_last {
            self.last_record = Some(seed.clone());
        }

        return sink(ReplayVisit::State(seed.clone(), state), &self.engine);
    }

    fn capture(
        &mut self,
        sink: &mut impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        if self.emitted == Some(self.position) {
            return Ok(true);
        }

        self.budget.charge_capture()?;

        let state = self.engine.snapshot()?;

        return self.emit_state(state, self.position, self.unit, sink);
    }

    fn capture_baseline(
        &mut self,
        monotonic_ns: u64,
        sink: &mut impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let mut state = self.engine.snapshot()?;
        state.dirty = DirtyState::Full;
        state.dirty_rows = (0..state.rows).collect();

        // Keep the warmed Ghostty engine, but forget emission history: the first ranged record
        // is a complete delta that does not depend on omitted states.
        self.previous = None;
        self.previous_hash = None;
        self.row_hashes.clear();

        return self.emit_state(
            state,
            self.position,
            Unit {
                event_seq: None,
                event_byte_offset: None,
                monotonic_ns,
            },
            sink,
        );
    }

    fn emit_state(
        &mut self,
        state: SemanticState,
        position: u64,
        unit: Unit,
        sink: &mut impl FnMut(ReplayVisit<'_>, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let hash = self.hash_state(&state)?;
        let delta = between(self.previous.as_ref(), &state);
        let record = StateRecord {
            state_id: position,
            event_seq: unit.event_seq,
            event_byte_offset: unit.event_byte_offset,
            monotonic_ns: unit.monotonic_ns,
            state_hash: hash.clone(),
            presented: !state.modes.synchronized_output,
            ghostty_dirty: state.dirty,
            ghostty_dirty_rows: state.dirty_rows.clone(),
            delta,
        };

        self.budget.charge_state()?;

        if self.keep_last {
            self.last_record = Some(record.clone());
        }

        let continue_replay = sink(ReplayVisit::State(record, &state), &self.engine)?;

        self.emitted = Some(position);
        self.previous = Some(state);
        self.previous_hash = Some(hash);

        return Ok(continue_replay);
    }

    fn hash_state(&mut self, state: &SemanticState) -> Result<String> {
        let reset_rows = self.row_hashes.len() != state.grid.len();
        if reset_rows {
            self.row_hashes = vec![blake3::hash(&[]); state.grid.len()];
        }

        for row in &state.grid {
            let changed = reset_rows
                || self
                    .previous
                    .as_ref()
                    .and_then(|previous| return previous.grid.get(usize::from(row.row)))
                    .is_none_or(|previous| {
                        return previous.row != row.row
                            || previous.soft_wrapped != row.soft_wrapped
                            || previous.wrap_continuation != row.wrap_continuation
                            || previous.cells != row.cells;
                    });

            if changed {
                let slot = self
                    .row_hashes
                    .get_mut(usize::from(row.row))
                    .ok_or_else(|| return invalid_replay("semantic row is outside the viewport"))?;
                *slot = hash_semantic_row(row).map_err(Error::ScreenFrame)?;
            }
        }

        return semantic_state_hash_from_row_hashes(state, &self.row_hashes)
            .map_err(Error::ScreenFrame);
    }
}

impl ReplayBudget {
    fn new(cols: u16, rows: u16, limits: ReplayLimits) -> Result<Self> {
        let current_cells = supported_geometry_cell_count(cols, rows)
            .ok_or_else(|| return invalid_replay("terminal geometry exceeds replay limits"))?;

        return Ok(Self {
            limits,
            started: Instant::now(),
            output_bytes: 0,
            capture_attempts: 0,
            state_count: 0,
            cell_work: 0,
            current_cells: u64::from(current_cells),
        });
    }

    fn check_elapsed(&self) -> Result<()> {
        if self.started.elapsed() > self.limits.elapsed {
            return Err(invalid_replay(format!(
                "replay exceeded {:?} elapsed-time limit",
                self.limits.elapsed
            )));
        }
        return Ok(());
    }

    fn charge_output(&mut self, bytes: usize) -> Result<()> {
        self.output_bytes = self
            .output_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));

        #[cfg(test)]
        TEST_REPLAY_OUTPUT_BYTES.set(
            TEST_REPLAY_OUTPUT_BYTES
                .get()
                .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX)),
        );

        if self.output_bytes > self.limits.output_bytes {
            return Err(invalid_replay(format!(
                "replay exceeded {} output-byte limit",
                self.limits.output_bytes
            )));
        }
        return self.check_elapsed();
    }

    fn set_geometry(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.current_cells = u64::from(
            supported_geometry_cell_count(cols, rows)
                .ok_or_else(|| return invalid_replay("resize geometry exceeds replay limits"))?,
        );
        return self.check_elapsed();
    }

    fn charge_capture(&mut self) -> Result<()> {
        self.capture_attempts = self.capture_attempts.saturating_add(1);
        self.cell_work = self.cell_work.saturating_add(self.current_cells);

        if self.capture_attempts > self.limits.capture_attempts {
            return Err(invalid_replay(format!(
                "replay exceeded {} capture-attempt limit",
                self.limits.capture_attempts
            )));
        }

        if self.cell_work > self.limits.cell_work {
            return Err(invalid_replay(format!(
                "replay exceeded {} cell-work limit",
                self.limits.cell_work
            )));
        }
        return self.check_elapsed();
    }

    fn charge_state(&mut self) -> Result<()> {
        self.state_count = self.state_count.saturating_add(1);

        if self.state_count > self.limits.state_count {
            return Err(invalid_replay(format!(
                "replay exceeded {} semantic-state limit",
                self.limits.state_count
            )));
        }
        return self.check_elapsed();
    }
}

fn cell_size(pixels: u16, cells: u16) -> u32 {
    return u32::from(pixels.checked_div(cells).unwrap_or(0).max(1));
}
