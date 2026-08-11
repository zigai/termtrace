use super::*;

#[cfg(test)]
use std::cell::Cell;

use crate::event::EventRecord;

const MAX_REPLAY_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REPLAY_CAPTURE_ATTEMPTS: u64 = 2_000_000;
const MAX_REPLAY_STATE_COUNT: u64 = 100_000;
const MAX_REPLAY_CELL_WORK: u64 = 2_000_000_000;
const MAX_REPLAY_ELAPSED: Duration = Duration::from_secs(60);

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

#[derive(Clone, Debug)]
pub(crate) struct ReplaySeed {
    pub inspection: StateInspection,
    pub engine_vt: Vec<u8>,
}

struct ReplayBuilder {
    engine: GhosttyVt,
    previous: Option<SemanticState>,
    previous_hash: Option<String>,
    pub(super) state_count: u64,
    row_hashes: Vec<blake3::Hash>,
    budget: ReplayBudget,
}

struct ReplayBudget {
    limits: ReplayLimits,
    started: Instant,
    pub(super) output_bytes: u64,
    pub(super) capture_attempts: u64,
    pub(super) state_count: u64,
    pub(super) cell_work: u64,
    current_cells: u64,
}

#[derive(Serialize)]
struct StateHashMetadata<'state> {
    cols: u16,
    rows: u16,
    cursor: &'state CursorState,
    modes: &'state ModeState,
    colors: &'state ColorState,
    active_style: &'state CellStyle,
    metadata: &'state TerminalMetadata,
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
    let metadata = StateHashMetadata {
        cols: state.cols,
        rows: state.rows,
        cursor: &state.cursor,
        modes: &state.modes,
        colors: &state.colors,
        active_style: &state.active_style,
        metadata: &state.metadata,
    };
    let encoded = serde_json::to_vec(&metadata)
        .map_err(|source| return json_error(PathBuf::from("<semantic-metadata>"), source))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"termtrace-semantic-state-v1\0");
    hash.update(&encoded);
    for row in &state.grid {
        let encoded = serde_json::to_vec(&row.cells)
            .map_err(|source| return json_error(PathBuf::from("<semantic-row>"), source))?;
        hash.update(blake3::hash(&encoded).as_bytes());
    }
    return Ok(hash.finalize().to_hex().to_string());
}

/// Replay a validated event log and collect every distinct state.
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

/// Visit every distinct semantic state with its complete reconstructed value.
///
/// This crate-private streaming interface lets derived renderers consume the
/// byte-granular replay without retaining every complete viewport in memory.
pub(crate) fn visit_states(
    log: &EventLog,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<()>,
) -> Result<()> {
    replay_for_inspection(log, None, |record, state, _| {
        visitor(&record, state)?;
        return Ok(());
    })?;
    return Ok(());
}

/// Visit semantic states in a validated source range using the ranged replay
/// baseline and dense state identifiers shared by derived indexes.
pub(crate) fn visit_states_range(
    log: &EventLog,
    source_range: Option<SourceRange>,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<()>,
) -> Result<()> {
    if let Some(range) = source_range
        && let Some(seed) =
            replay_seed_for_selector(log, &StateSelector::MonotonicNs(range.start_monotonic_ns))?
    {
        return visit_states_range_seeded(log, range, seed, visitor);
    }
    replay_for_inspection(log, source_range, |record, state, _| {
        visitor(&record, state)?;
        return Ok(());
    })?;
    return Ok(());
}

pub(crate) fn visit_states_range_seeded(
    log: &EventLog,
    source_range: SourceRange,
    seed: ReplaySeed,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<()>,
) -> Result<()> {
    replay_with_seed(log, Some(source_range), seed, |record, state, _| {
        visitor(&record, state)?;
        return Ok(());
    })?;
    return Ok(());
}

pub(crate) fn visit_states_range_while(
    log: &EventLog,
    source_range: Option<SourceRange>,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<bool>,
) -> Result<()> {
    let seed = match source_range {
        Some(range) => {
            replay_seed_for_selector(log, &StateSelector::MonotonicNs(range.start_monotonic_ns))?
        }
        None => None,
    };
    replay_with_limits_and_formats_control(
        log,
        source_range,
        DEFAULT_REPLAY_LIMITS,
        false,
        seed,
        |record, state, _| return visitor(&record, state),
    )?;
    return Ok(());
}

pub(crate) fn visit_states_seeded_while(
    log: &EventLog,
    seed: ReplaySeed,
    mut visitor: impl FnMut(&StateRecord, &SemanticState) -> Result<bool>,
) -> Result<()> {
    replay_with_limits_and_formats_control(
        log,
        None,
        DEFAULT_REPLAY_LIMITS,
        false,
        Some(seed),
        |record, state, _| return visitor(&record, state),
    )?;
    return Ok(());
}

/// Reconstruct one state selected by state, event, time, or final position.
///
/// # Errors
///
/// Returns an error when the source cannot replay or no state matches.
pub fn inspect(log: &EventLog, selector: StateSelector) -> Result<StateInspection> {
    if let Some(seed) = replay_seed_for_selector(log, &selector)? {
        return inspect_seeded(log, selector, seed);
    }
    return inspect_many(log, &[selector])?
        .into_iter()
        .next()
        .ok_or_else(|| return invalid_replay("no semantic state matches the selector"));
}

/// Reconstruct several independently selected states in one canonical replay.
///
/// # Errors
///
/// Returns an error when replay fails or any selector has no matching state.
pub fn inspect_many(log: &EventLog, selectors: &[StateSelector]) -> Result<Vec<StateInspection>> {
    if selectors.is_empty() {
        return Ok(Vec::new());
    }
    let mut selected = vec![None; selectors.len()];
    replay_for_inspection(log, None, |record, state, _| {
        for (index, selector) in selectors.iter().enumerate() {
            if state_selector_matches(selector, &record) {
                let slot = selected.get_mut(index).ok_or_else(|| {
                    return invalid_replay("state selector index exceeded result storage");
                })?;
                *slot = Some(StateInspection {
                    record: record.clone(),
                    state: state.clone(),
                });
            }
        }
        return Ok(());
    })?;
    return selected
        .into_iter()
        .enumerate()
        .map(|(index, inspection)| {
            return inspection.ok_or_else(|| {
                return invalid_replay(format!(
                    "no semantic state matches selector at index {index}"
                ));
            });
        })
        .collect();
}

pub(crate) fn inspect_latest_presented_fast(log: &EventLog) -> Result<EndpointInspection> {
    use crate::record::presentation::{PresentationAction, PresentationTracker};

    let mut engine = GhosttyVt::new(log.header.cols, log.header.rows, 100_000)?;
    engine.apply_terminal_colors(&log.header.colors)?;
    let mut presented = engine.snapshot()?;
    let mut tracker = PresentationTracker::new(presented.modes.synchronized_output);
    let mut resize_intents: HashMap<Uuid, ResizeGeometry> = HashMap::new();
    let mut output_bytes = 0_u64;
    let mut presented_event_seq = None;
    let mut presented_event_byte_offset = None;
    let mut presented_monotonic_ns = None;
    let mut last_applied_source = (None, None, None);
    let mut last_buffered_source = (None, None, None);

    for record in &log.events {
        match &record.event {
            Event::Output { data } => {
                output_bytes = output_bytes
                    .checked_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX))
                    .ok_or_else(|| return invalid_replay("endpoint replay output byte overflow"))?;
                if output_bytes > MAX_REPLAY_OUTPUT_BYTES {
                    return Err(invalid_replay(format!(
                        "endpoint replay exceeded {MAX_REPLAY_OUTPUT_BYTES} output bytes"
                    )));
                }
                for (offset, byte) in data.as_bytes().iter().enumerate() {
                    match tracker.push(*byte) {
                        PresentationAction::Immediate => {
                            engine.write(slice::from_ref(byte));
                            drop(engine.take_responses());
                            last_applied_source = (
                                Some(record.seq),
                                Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                                Some(record.monotonic_ns),
                            );
                        }
                        PresentationAction::Buffered => {
                            last_buffered_source = (
                                Some(record.seq),
                                Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                                Some(record.monotonic_ns),
                            );
                        }
                        PresentationAction::Sequence { bytes, command } => {
                            if command == Some(true) {
                                let before = engine.snapshot()?;
                                if !before.modes.synchronized_output {
                                    presented = before;
                                    (
                                        presented_event_seq,
                                        presented_event_byte_offset,
                                        presented_monotonic_ns,
                                    ) = last_applied_source;
                                }
                            }
                            engine.write(&bytes);
                            drop(engine.take_responses());
                            last_applied_source = (
                                Some(record.seq),
                                Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                                Some(record.monotonic_ns),
                            );
                            if command == Some(false) {
                                let after = engine.snapshot()?;
                                if !after.modes.synchronized_output {
                                    presented = after;
                                    (
                                        presented_event_seq,
                                        presented_event_byte_offset,
                                        presented_monotonic_ns,
                                    ) = last_applied_source;
                                }
                            }
                        }
                    }
                }
                let latest = engine.snapshot()?;
                if !latest.modes.synchronized_output {
                    presented = latest;
                    (
                        presented_event_seq,
                        presented_event_byte_offset,
                        presented_monotonic_ns,
                    ) = last_applied_source;
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
                engine.resize(
                    geometry.cols,
                    geometry.rows,
                    cell_size(geometry.pixel_width, geometry.cols),
                    cell_size(geometry.pixel_height, geometry.rows),
                )?;
                let latest = engine.snapshot()?;
                last_applied_source = (Some(record.seq), None, Some(record.monotonic_ns));
                if !latest.modes.synchronized_output {
                    presented = latest;
                    (
                        presented_event_seq,
                        presented_event_byte_offset,
                        presented_monotonic_ns,
                    ) = last_applied_source;
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }

    let pending = tracker.finish();
    if !pending.is_empty() {
        engine.write(&pending);
        last_applied_source = last_buffered_source;
        drop(engine.take_responses());
        let latest = engine.snapshot()?;
        if !latest.modes.synchronized_output {
            presented = latest;
            (
                presented_event_seq,
                presented_event_byte_offset,
                presented_monotonic_ns,
            ) = last_applied_source;
        }
    }
    let state_hash = semantic_state_hash(&presented)?;
    return Ok(EndpointInspection {
        state: presented,
        state_hash,
        event_seq: presented_event_seq,
        event_byte_offset: presented_event_byte_offset,
        monotonic_ns: presented_monotonic_ns,
        presented: true,
    });
}

pub(crate) fn inspect_seeded(
    log: &EventLog,
    selector: StateSelector,
    seed: ReplaySeed,
) -> Result<StateInspection> {
    let mut selected = None;
    replay_with_seed(log, None, seed, |record, state, _| {
        if state_selector_matches(&selector, &record) {
            selected = Some(StateInspection {
                record,
                state: state.clone(),
            });
        }
        return Ok(());
    })?;
    return selected.ok_or_else(|| return invalid_replay("no semantic state matches the selector"));
}

/// Format one selected state through the pinned Ghostty formatter.
///
/// # Errors
///
/// Returns an error when replay, selection, or formatter output fails.
pub fn format_state(log: &EventLog, selector: StateSelector, format: Format) -> Result<Vec<u8>> {
    let mut selected = None;
    replay_for_selector(log, &selector, |record, _, engine| {
        if state_selector_matches(&selector, &record) {
            selected = Some(engine.format(format)?);
        }
        return Ok(());
    })?;
    return selected.ok_or_else(|| return invalid_replay("no semantic state matches the selector"));
}

/// Engine-native format bytes paired with their requested format.
pub type RenderedStateFormats = Vec<(Format, Vec<u8>)>;

/// One selected state and all requested engine-native representations.
pub type StateInspectionWithFormats = (StateInspection, RenderedStateFormats);

/// Reconstruct one selected state and requested engine-native formats in one replay.
///
/// # Errors
///
/// Returns an error when replay, selection, or formatting fails.
pub fn inspect_with_formats(
    log: &EventLog,
    selector: StateSelector,
    formats: &[Format],
) -> Result<StateInspectionWithFormats> {
    let mut selected = None;
    let mut rendered = Vec::new();
    replay_for_selector(log, &selector, |record, state, engine| {
        if state_selector_matches(&selector, &record) {
            selected = Some(StateInspection {
                record,
                state: state.clone(),
            });
            rendered.clear();
            for format in formats {
                rendered.push((*format, engine.format(*format)?));
            }
        }
        return Ok(());
    })?;
    let inspection =
        selected.ok_or_else(|| return invalid_replay("no semantic state matches the selector"))?;
    return Ok((inspection, rendered));
}

fn replay_seed_for_selector(
    log: &EventLog,
    selector: &StateSelector,
) -> Result<Option<ReplaySeed>> {
    let Some(event_path) = log.source_path.as_ref() else {
        return Ok(None);
    };
    let Some(session) = event_path.parent() else {
        return Ok(None);
    };
    let directory = session.join(DERIVED_DIRECTORY);
    if !directory.is_dir() {
        return Ok(None);
    }
    return load_nearest_replay_seed(&directory, log, selector);
}

fn replay_for_selector(
    log: &EventLog,
    selector: &StateSelector,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    if let Some(seed) = replay_seed_for_selector(log, selector)? {
        return replay_with_seed(log, None, seed, state_sink);
    }
    return replay_for_inspection(log, None, state_sink);
}

fn state_selector_matches(selector: &StateSelector, record: &StateRecord) -> bool {
    return match selector {
        StateSelector::Final => true,
        StateSelector::LatestPresented => record.presented,
        StateSelector::StateId(id) => record.state_id == *id,
        StateSelector::StateHash(hash) => record.state_hash == *hash,
        StateSelector::EventSeq(seq) => record.event_seq.is_none_or(|event| return event <= *seq),
        StateSelector::MonotonicNs(time) => record.monotonic_ns <= *time,
    };
}

pub(super) fn replay_with(
    log: &EventLog,
    source_range: Option<SourceRange>,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits(log, source_range, DEFAULT_REPLAY_LIMITS, state_sink);
}

pub(super) fn replay_for_index(
    log: &EventLog,
    source_range: Option<SourceRange>,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits(log, source_range, INDEX_REPLAY_LIMITS, state_sink);
}

pub(crate) fn replay_with_seed(
    log: &EventLog,
    source_range: Option<SourceRange>,
    seed: ReplaySeed,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits_and_formats(
        log,
        source_range,
        DEFAULT_REPLAY_LIMITS,
        false,
        Some(seed),
        state_sink,
    );
}

fn replay_for_inspection(
    log: &EventLog,
    source_range: Option<SourceRange>,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits_and_formats(
        log,
        source_range,
        DEFAULT_REPLAY_LIMITS,
        false,
        None,
        state_sink,
    );
}

pub(super) fn replay_with_limits(
    log: &EventLog,
    source_range: Option<SourceRange>,
    limits: ReplayLimits,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits_and_formats(log, source_range, limits, true, None, state_sink);
}

#[cfg(test)]
pub(super) fn replay_with_seed_and_limits(
    log: &EventLog,
    source_range: Option<SourceRange>,
    seed: ReplaySeed,
    limits: ReplayLimits,
    state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits_and_formats(
        log,
        source_range,
        limits,
        false,
        Some(seed),
        state_sink,
    );
}

pub(crate) fn validate_replay_seed(log: &EventLog, seed: &ReplaySeed) -> Result<()> {
    drop(ReplayBuilder::from_seed(log, DEFAULT_REPLAY_LIMITS, seed)?);
    return Ok(());
}

fn replay_with_limits_and_formats(
    log: &EventLog,
    source_range: Option<SourceRange>,
    limits: ReplayLimits,
    include_final_formats: bool,
    seed: Option<ReplaySeed>,
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    return replay_with_limits_and_formats_control(
        log,
        source_range,
        limits,
        include_final_formats,
        seed,
        |record, state, engine| {
            state_sink(record, state, engine)?;
            return Ok(true);
        },
    );
}

fn replay_with_limits_and_formats_control(
    log: &EventLog,
    source_range: Option<SourceRange>,
    limits: ReplayLimits,
    include_final_formats: bool,
    seed: Option<ReplaySeed>,
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<bool>,
) -> Result<ReplayArtifacts> {
    let mut builder = match seed.as_ref() {
        Some(seed) => ReplayBuilder::from_seed(log, limits, seed)?,
        None => ReplayBuilder::new(log, limits)?,
    };
    let mut resize_intents = HashMap::new();
    let mut continue_replay = true;
    if source_range.is_none() {
        continue_replay = if let Some(seed) = seed.as_ref() {
            state_sink(
                seed.inspection.record.clone(),
                &seed.inspection.state,
                &builder.engine,
            )?
        } else {
            builder.capture(None, None, 0, &mut state_sink)?
        };
    }
    let mut emitted_range_baseline = false;
    let seed_record = seed.as_ref().map(|seed| return &seed.inspection.record);
    if let (Some(range), Some(record)) = (source_range, seed_record)
        && record.monotonic_ns > range.start_monotonic_ns
    {
        return Err(invalid_replay(
            "replay checkpoint is later than the requested source range",
        ));
    }
    'events: for record in &log.events {
        if !continue_replay {
            break;
        }
        let seed_byte_start = source_byte_start(record, seed_record, &mut resize_intents)?;
        let Some(seed_byte_start) = seed_byte_start else {
            continue;
        };
        let emit = source_range.is_none_or(|range| return range.contains(record.monotonic_ns));
        if source_range.is_some_and(|range| return record.monotonic_ns >= range.end_monotonic_ns) {
            break;
        }
        if !emitted_range_baseline
            && source_range
                .is_some_and(|range| return record.monotonic_ns >= range.start_monotonic_ns)
        {
            let range = source_range.ok_or_else(|| {
                return invalid_replay("range baseline requested without a source range");
            })?;
            // Capture before applying an event exactly at `start`. This preserves
            // the [start, end) contract: state 0 is the boundary state, and the
            // event at the inclusive boundary remains an observable transition.
            continue_replay =
                builder.capture_baseline(range.start_monotonic_ns, &mut state_sink)?;
            emitted_range_baseline = true;
            if !continue_replay {
                break;
            }
        }
        match &record.event {
            Event::Output { data } => {
                let bytes = data.as_bytes().get(seed_byte_start..).ok_or_else(|| {
                    return invalid_replay("replay checkpoint output cursor is outside its event");
                })?;
                builder.budget.charge_output(bytes.len())?;
                for (relative_offset, byte) in bytes.iter().enumerate() {
                    if relative_offset % 4_096 == 0 {
                        builder.budget.check_elapsed()?;
                    }
                    let offset = seed_byte_start
                        .checked_add(relative_offset)
                        .ok_or_else(|| return invalid_replay("output byte offset overflow"))?;
                    builder.engine.write(slice::from_ref(byte));
                    drop(builder.engine.take_responses());
                    if emit {
                        continue_replay = builder.capture(
                            Some(record.seq),
                            Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                            record.monotonic_ns,
                            &mut state_sink,
                        )?;
                        if !continue_replay {
                            break 'events;
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
                builder.engine.resize(
                    geometry.cols,
                    geometry.rows,
                    cell_size(geometry.pixel_width, geometry.cols),
                    cell_size(geometry.pixel_height, geometry.rows),
                )?;
                builder.budget.set_geometry(geometry.cols, geometry.rows)?;
                if emit {
                    continue_replay = builder.capture(
                        Some(record.seq),
                        None,
                        record.monotonic_ns,
                        &mut state_sink,
                    )?;
                    if !continue_replay {
                        break 'events;
                    }
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }
    if continue_replay
        && let Some(range) = source_range
        && !emitted_range_baseline
    {
        builder.capture_baseline(range.start_monotonic_ns, &mut state_sink)?;
    }
    let final_state_hash = builder
        .previous_hash
        .clone()
        .ok_or_else(|| return invalid_replay("replay produced no initial state"))?;
    let (final_text, final_vt, final_html) = if include_final_formats {
        (
            builder.engine.format(Format::Plain)?,
            builder.engine.format(Format::Vt)?,
            builder.engine.format(Format::Html)?,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    return Ok(ReplayArtifacts {
        state_count: builder.state_count,
        final_state_hash,
        final_text,
        final_vt,
        final_html,
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

fn append_sgr_color(sequence: &mut String, prefix: u8, color: ColorSpec) -> Result<()> {
    match color {
        ColorSpec::None => {}
        ColorSpec::Palette(index) => {
            write!(sequence, ";{prefix};5;{index}")
                .map_err(|_| return invalid_replay("format checkpoint palette style"))?;
        }
        ColorSpec::Rgb(color) => {
            write!(sequence, ";{prefix};2;{};{};{}", color.r, color.g, color.b)
                .map_err(|_| return invalid_replay("format checkpoint RGB style"))?;
        }
    }
    return Ok(());
}

fn checkpoint_sgr(style: &CellStyle) -> Result<String> {
    let mut sequence = "\x1b[0".to_owned();
    for (enabled, code) in [
        (style.bold, 1),
        (style.faint, 2),
        (style.italic, 3),
        (style.blink, 5),
        (style.inverse, 7),
        (style.invisible, 8),
        (style.strikethrough, 9),
        (style.overline, 53),
    ] {
        if enabled {
            write!(sequence, ";{code}")
                .map_err(|_| return invalid_replay("format checkpoint SGR attribute"))?;
        }
    }
    if style.underline != 0 {
        write!(sequence, ";4:{}", style.underline)
            .map_err(|_| return invalid_replay("format checkpoint underline style"))?;
    }
    append_sgr_color(&mut sequence, 38, style.foreground)?;
    append_sgr_color(&mut sequence, 48, style.background)?;
    append_sgr_color(&mut sequence, 58, style.underline_color)?;
    sequence.push('m');
    return Ok(sequence);
}

fn restore_checkpoint_grid(
    engine: &mut GhosttyVt,
    expected: &SemanticState,
    actual: &SemanticState,
) -> Result<()> {
    engine.write(b"\x1b[?7l");
    for (expected_row, actual_row) in expected.grid.iter().zip(&actual.grid) {
        let mut previous_non_ascii = false;
        for (expected_cell, actual_cell) in expected_row.cells.iter().zip(&actual_row.cells) {
            if expected_cell == actual_cell {
                previous_non_ascii = expected_cell
                    .grapheme
                    .chars()
                    .any(|character| return !character.is_ascii());
                continue;
            }
            if expected_cell.grapheme.is_empty() && previous_non_ascii {
                previous_non_ascii = false;
                continue;
            }
            let cursor = format!(
                "\x1b[{};{}H",
                u32::from(expected_row.row).saturating_add(1),
                u32::from(expected_cell.column).saturating_add(1)
            );
            engine.write(cursor.as_bytes());
            engine.write(checkpoint_sgr(&expected_cell.style)?.as_bytes());
            if expected_cell.grapheme.is_empty() {
                engine.write(b" ");
            } else {
                engine.write(expected_cell.grapheme.as_bytes());
            }
            previous_non_ascii = expected_cell
                .grapheme
                .chars()
                .any(|character| return !character.is_ascii());
        }
    }
    engine.write(if expected.modes.wraparound {
        b"\x1b[?7h"
    } else {
        b"\x1b[?7l"
    });
    engine.write(checkpoint_sgr(&expected.active_style)?.as_bytes());
    if let (Some(row), Some(column)) = (expected.cursor.row, expected.cursor.column) {
        let cursor = format!(
            "\x1b[{};{}H",
            u32::from(row).saturating_add(1),
            u32::from(column).saturating_add(1)
        );
        engine.write(cursor.as_bytes());
    }
    return Ok(());
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

impl ReplayBuilder {
    fn new(log: &EventLog, limits: ReplayLimits) -> Result<Self> {
        let mut engine = GhosttyVt::new(log.header.cols, log.header.rows, 100_000)?;
        engine.apply_terminal_colors(&log.header.colors)?;
        return Ok(Self {
            engine,
            previous: None,
            previous_hash: None,
            state_count: 0,
            row_hashes: Vec::new(),
            budget: ReplayBudget::new(log.header.cols, log.header.rows, limits)?,
        });
    }

    fn from_seed(log: &EventLog, limits: ReplayLimits, seed: &ReplaySeed) -> Result<Self> {
        let record = &seed.inspection.record;
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
        let state = &seed.inspection.state;
        if semantic_state_hash(state)? != record.state_hash {
            return Err(invalid_replay(
                "replay checkpoint semantic state hash is invalid",
            ));
        }
        let mut engine = GhosttyVt::new(state.cols, state.rows, 100_000)?;
        engine.apply_terminal_colors(&log.header.colors)?;
        engine.write(&seed.engine_vt);
        drop(engine.take_responses());
        engine.write(b"\x1b]2;");
        engine.write(state.metadata.title.as_bytes());
        engine.write(b"\x1b\\");
        engine.write(b"\x1b]7;");
        engine.write(state.metadata.working_directory.as_bytes());
        engine.write(b"\x1b\\");
        if let (Some(row), Some(column)) = (state.cursor.row, state.cursor.column) {
            let cursor_position = format!(
                "\x1b[{};{}H",
                u32::from(row).saturating_add(1),
                u32::from(column).saturating_add(1)
            );
            engine.write(cursor_position.as_bytes());
        }
        let mut restored = engine.snapshot()?;
        if restored.grid.len() != state.grid.len()
            || restored
                .grid
                .iter()
                .zip(&state.grid)
                .any(|(actual, expected)| return actual.cells != expected.cells)
        {
            restore_checkpoint_grid(&mut engine, state, &restored)?;
            restored = engine.snapshot()?;
        }
        if semantic_state_hash(&restored)? != record.state_hash {
            let mut mismatches: Vec<&str> = Vec::new();
            if restored.grid.len() != state.grid.len()
                || restored
                    .grid
                    .iter()
                    .zip(&state.grid)
                    .any(|(actual, expected)| return actual.cells != expected.cells)
            {
                mismatches.push("grid");
            }
            if restored.cursor != state.cursor {
                mismatches.push("cursor");
            }
            if restored.modes != state.modes {
                mismatches.push("modes");
            }
            if restored.colors != state.colors {
                mismatches.push("colors");
            }
            if restored.active_style != state.active_style {
                mismatches.push("active_style");
            }
            if restored.metadata != state.metadata {
                mismatches.push("metadata");
            }
            return Err(invalid_replay(format!(
                "replay checkpoint VT seed does not reconstruct semantic fields: {}",
                mismatches.join(", ")
            )));
        }
        let state_count = record
            .state_id
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("replay checkpoint state identifier overflow"))?;
        let mut builder = Self {
            engine,
            previous: None,
            previous_hash: None,
            state_count,
            row_hashes: Vec::new(),
            budget: ReplayBudget::new(state.cols, state.rows, limits)?,
        };
        let restored_hash = builder.hash_state(&restored)?;
        builder.previous = Some(restored);
        builder.previous_hash = Some(restored_hash);
        return Ok(builder);
    }

    fn capture(
        &mut self,
        event_seq: Option<u64>,
        event_byte_offset: Option<u64>,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        self.budget.charge_capture()?;
        let Some(state) = self.engine.snapshot_if_changed()? else {
            return Ok(true);
        };
        return self.emit_state(state, event_seq, event_byte_offset, monotonic_ns, sink);
    }

    fn capture_baseline(
        &mut self,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let mut state = self.engine.snapshot()?;
        state.dirty = DirtyState::Full;
        state.dirty_rows = (0..state.rows).collect();
        // Keep the warmed Ghostty engine, but forget emission history. The first
        // ranged record must be a complete delta with dense state ID 0; carrying
        // prefix hashes here would either suppress it or make it depend on omitted states.
        self.previous = None;
        self.previous_hash = None;
        self.state_count = 0;
        self.row_hashes.clear();
        return self.emit_state(state, None, None, monotonic_ns, sink);
    }

    fn emit_state(
        &mut self,
        state: SemanticState,
        event_seq: Option<u64>,
        event_byte_offset: Option<u64>,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<bool>,
    ) -> Result<bool> {
        let hash = self.hash_state(&state)?;
        if self.previous_hash.as_ref() == Some(&hash) {
            return Ok(true);
        }
        let delta = state_delta(self.previous.as_ref(), &state);
        let record = StateRecord {
            state_id: self.state_count,
            event_seq,
            event_byte_offset,
            monotonic_ns,
            state_hash: hash.clone(),
            presented: !state.modes.synchronized_output,
            ghostty_dirty: state.dirty,
            ghostty_dirty_rows: state.dirty_rows.clone(),
            delta,
        };
        self.budget.charge_state()?;
        let continue_replay = sink(record, &state, &self.engine)?;
        self.state_count = self
            .state_count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("semantic state identifier overflow"))?;
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
                    .is_none_or(|previous| return previous.cells != row.cells);
            if changed {
                let encoded = serde_json::to_vec(&row.cells)
                    .map_err(|source| return json_error(PathBuf::from("<semantic-row>"), source))?;
                let slot = self
                    .row_hashes
                    .get_mut(usize::from(row.row))
                    .ok_or_else(|| return invalid_replay("semantic row is outside the viewport"))?;
                *slot = blake3::hash(&encoded);
            }
        }
        let metadata = StateHashMetadata {
            cols: state.cols,
            rows: state.rows,
            cursor: &state.cursor,
            modes: &state.modes,
            colors: &state.colors,
            active_style: &state.active_style,
            metadata: &state.metadata,
        };
        let encoded = serde_json::to_vec(&metadata)
            .map_err(|source| return json_error(PathBuf::from("<semantic-metadata>"), source))?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"termtrace-semantic-state-v1\0");
        hash.update(&encoded);
        for row_hash in &self.row_hashes {
            hash.update(row_hash.as_bytes());
        }
        return Ok(hash.finalize().to_hex().to_string());
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
