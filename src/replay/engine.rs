use super::*;

const MAX_REPLAY_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REPLAY_CAPTURE_ATTEMPTS: u64 = 2_000_000;
const MAX_REPLAY_STATE_COUNT: u64 = 100_000;
const MAX_REPLAY_CELL_WORK: u64 = 2_000_000_000;
const MAX_REPLAY_EMITTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REPLAY_ELAPSED: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
pub(super) struct ReplayLimits {
    pub(super) output_bytes: u64,
    pub(super) capture_attempts: u64,
    pub(super) state_count: u64,
    pub(super) cell_work: u64,
    pub(super) emitted_bytes: u64,
    pub(super) elapsed: Duration,
}

pub(super) const DEFAULT_REPLAY_LIMITS: ReplayLimits = ReplayLimits {
    output_bytes: MAX_REPLAY_OUTPUT_BYTES,
    capture_attempts: MAX_REPLAY_CAPTURE_ATTEMPTS,
    state_count: MAX_REPLAY_STATE_COUNT,
    cell_work: MAX_REPLAY_CELL_WORK,
    emitted_bytes: MAX_REPLAY_EMITTED_BYTES,
    elapsed: MAX_REPLAY_ELAPSED,
};

#[derive(Clone, Copy)]
struct ResizeGeometry {
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
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
    pub(super) emitted_bytes: u64,
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
) -> Result<ReplayArtifacts> {
    return replay_with(log, None, |record, state, _| {
        visitor(&record, state)?;
        return Ok(());
    });
}

/// Reconstruct one state selected by state, event, time, or final position.
///
/// # Errors
///
/// Returns an error when the source cannot replay or no state matches.
pub fn inspect(log: &EventLog, selector: StateSelector) -> Result<StateInspection> {
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
    replay_with(log, None, |record, state, _| {
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

/// Format one selected state through the pinned Ghostty formatter.
///
/// # Errors
///
/// Returns an error when replay, selection, or formatter output fails.
pub fn format_state(log: &EventLog, selector: StateSelector, format: Format) -> Result<Vec<u8>> {
    let mut selected = None;
    replay_with(log, None, |record, _, engine| {
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
    replay_with(log, None, |record, state, engine| {
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

pub(super) fn replay_with_limits(
    log: &EventLog,
    source_range: Option<SourceRange>,
    limits: ReplayLimits,
    mut state_sink: impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let mut builder = ReplayBuilder::new(log, limits)?;
    let mut resize_intents = HashMap::new();
    if source_range.is_none() {
        builder.capture(None, None, 0, &mut state_sink)?;
    }
    // Ranged replay suppresses prefix snapshots, not prefix execution. Output,
    // resize intents, and their later results must still cross the boundary so
    // the baseline at `start` represents the same terminal the user actually saw.
    let mut emitted_range_baseline = false;
    for record in &log.events {
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
            builder.capture_baseline(range.start_monotonic_ns, &mut state_sink)?;
            emitted_range_baseline = true;
        }
        match &record.event {
            Event::Output { data } => {
                builder.budget.charge_output(data.as_bytes().len())?;
                for (offset, byte) in data.as_bytes().iter().enumerate() {
                    if offset % 4_096 == 0 {
                        builder.budget.check_elapsed()?;
                    }
                    builder.engine.write(slice::from_ref(byte));
                    drop(builder.engine.take_responses());
                    if emit {
                        builder.capture(
                            Some(record.seq),
                            Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                            record.monotonic_ns,
                            &mut state_sink,
                        )?;
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
                    builder.capture(
                        Some(record.seq),
                        None,
                        record.monotonic_ns,
                        &mut state_sink,
                    )?;
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }
    if let Some(range) = source_range
        && !emitted_range_baseline
    {
        builder.capture_baseline(range.start_monotonic_ns, &mut state_sink)?;
    }
    let final_state_hash = builder
        .previous_hash
        .clone()
        .ok_or_else(|| return invalid_replay("replay produced no initial state"))?;
    let final_text = builder.engine.format(Format::Plain)?;
    let final_vt = builder.engine.format(Format::Vt)?;
    let final_html = builder.engine.format(Format::Html)?;
    builder.budget.charge_emitted(
        final_text
            .len()
            .saturating_add(final_vt.len())
            .saturating_add(final_html.len()),
    )?;
    return Ok(ReplayArtifacts {
        state_count: builder.state_count,
        final_state_hash,
        final_text,
        final_vt,
        final_html,
    });
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

    fn capture(
        &mut self,
        event_seq: Option<u64>,
        event_byte_offset: Option<u64>,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
    ) -> Result<()> {
        self.budget.charge_capture()?;
        let Some(state) = self.engine.snapshot_if_changed()? else {
            return Ok(());
        };
        return self.emit_state(state, event_seq, event_byte_offset, monotonic_ns, sink);
    }

    fn capture_baseline(
        &mut self,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
    ) -> Result<()> {
        self.budget.charge_capture()?;
        let state = self.engine.snapshot()?;
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
        sink: &mut impl FnMut(StateRecord, &SemanticState, &GhosttyVt) -> Result<()>,
    ) -> Result<()> {
        let hash = self.hash_state(&state)?;
        if self.previous_hash.as_ref() == Some(&hash) {
            return Ok(());
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
        self.budget.charge_state(&record)?;
        sink(record, &state, &self.engine)?;
        self.state_count = self
            .state_count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("semantic state identifier overflow"))?;
        self.previous = Some(state);
        self.previous_hash = Some(hash);
        return Ok(());
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
            emitted_bytes: 0,
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

    fn charge_state(&mut self, record: &StateRecord) -> Result<()> {
        self.state_count = self.state_count.saturating_add(1);
        if self.state_count > self.limits.state_count {
            return Err(invalid_replay(format!(
                "replay exceeded {} semantic-state limit",
                self.limits.state_count
            )));
        }
        let mut counter = CountingWriter::default();
        serde_json::to_writer(&mut counter, record)
            .map_err(|source| return json_error(PathBuf::from("<replay-budget>"), source))?;
        return self.charge_emitted(counter.bytes);
    }

    fn charge_emitted(&mut self, bytes: usize) -> Result<()> {
        self.emitted_bytes = self
            .emitted_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if self.emitted_bytes > self.limits.emitted_bytes {
            return Err(invalid_replay(format!(
                "replay exceeded {} emitted-byte limit",
                self.limits.emitted_bytes
            )));
        }
        return self.check_elapsed();
    }
}

#[derive(Default)]
struct CountingWriter {
    bytes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.bytes = self.bytes.saturating_add(buf.len());
        return Ok(buf.len());
    }

    fn flush(&mut self) -> IoResult<()> {
        return Ok(());
    }
}

fn cell_size(pixels: u16, cells: u16) -> u32 {
    return u32::from(pixels.checked_div(cells).unwrap_or(0).max(1));
}
