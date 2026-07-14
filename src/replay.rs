//! Deterministic byte-granular replay and semantic state indexing.

use std::{
    collections::HashMap,
    fmt::Write as _,
    io::Write,
    path::{Path, PathBuf},
    slice,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
        sync_directory, write_secure,
    },
    control::{ControlCommand, ControlData, request},
    event::{Event, OperationStatus},
    event_log::EventLog,
    ghostty_vt::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, Format, GhosttyVt, ModeState,
        SemanticState, TerminalMetadata,
    },
    session::validate_completed_log,
    timeline::{SourceRange, resolve_source_range},
};

/// Pinned Ghostty source revision used by the semantic adapter.
pub const GHOSTTY_REVISION: &str = "55a3e33ab26a23d75b274b23c7f76d837db00578";
/// Derived artifact format version.
pub const STATE_FORMAT_VERSION: u32 = 2;
/// Relative directory containing replay-derived artifacts.
pub const DERIVED_DIRECTORY: &str = "derived";

/// Semantic states retained in one derived index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateSelection {
    /// Retain every distinct state, including synchronized-output internals.
    #[default]
    All,
    /// Retain only states eligible for presentation by a native terminal.
    Presented,
}

/// Policy for one semantic indexing operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IndexOptions {
    /// Optional inclusive source offset.
    pub start_monotonic_ns: Option<u64>,
    /// Optional exclusive source offset.
    pub end_monotonic_ns: Option<u64>,
    /// Permit a hash-addressed snapshot when completion is absent.
    pub live_snapshot: bool,
    /// Semantic states retained in artifacts.
    pub state_selection: StateSelection,
}

/// Fine-grained changes for one viewport row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RowDiff {
    /// Zero-based row.
    pub row: u16,
    /// Cells whose complete semantic value changed.
    pub cells: Vec<CellState>,
}

/// Delta from the previous distinct state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateDelta {
    /// New `(cols, rows)` when geometry changed.
    pub geometry: Option<(u16, u16)>,
    /// New cursor when it changed.
    pub cursor: Option<CursorState>,
    /// New terminal modes when they changed.
    pub modes: Option<ModeState>,
    /// New colors and palette when they changed.
    pub colors: Option<ColorState>,
    /// New active SGR style when it changed.
    pub active_style: Option<CellStyle>,
    /// New terminal metadata when it changed.
    pub metadata: Option<TerminalMetadata>,
    /// Cell-level row deltas.
    pub rows: Vec<RowDiff>,
}

/// One distinct semantic state boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateRecord {
    /// Dense zero-based state identifier.
    pub state_id: u64,
    /// Canonical event responsible for this state; absent for an initial or range-baseline state.
    pub event_seq: Option<u64>,
    /// Zero-based byte offset inside an output event.
    pub event_byte_offset: Option<u64>,
    /// Monotonic event timestamp in nanoseconds.
    pub monotonic_ns: u64,
    /// BLAKE3 of the complete normalized semantic state.
    pub state_hash: String,
    /// Whether this state was eligible for presentation.
    pub presented: bool,
    /// Ghostty's global dirty classification.
    pub ghostty_dirty: DirtyState,
    /// Rows Ghostty marked dirty.
    pub ghostty_dirty_rows: Vec<u16>,
    /// Stable delta from the preceding distinct state.
    pub delta: StateDelta,
}

/// Compact event/video mapping information for one state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateIndexEntry {
    /// State identifier.
    pub state_id: u64,
    /// State hash.
    pub state_hash: String,
    /// Source event.
    pub event_seq: Option<u64>,
    /// Source event byte offset.
    pub event_byte_offset: Option<u64>,
    /// Monotonic state time.
    pub monotonic_ns: u64,
    /// Whether a native renderer could present this state.
    pub presented: bool,
}

/// Summary of one deterministic replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayArtifacts {
    /// Number of distinct states, including the initial blank state.
    pub state_count: u64,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
    /// Ghostty-formatted final plain text.
    pub final_text: Vec<u8>,
    /// Ghostty-formatted final VT reconstruction.
    pub final_vt: Vec<u8>,
    /// Ghostty-formatted final HTML.
    pub final_html: Vec<u8>,
}

/// In-memory replay result intended for tests and programmatic inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectedReplay {
    /// Every distinct state delta.
    pub states: Vec<StateRecord>,
    /// Final derived outputs.
    pub artifacts: ReplayArtifacts,
}

/// Selector for one reconstructed semantic state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateSelector {
    /// Final distinct state.
    Final,
    /// Exact state identifier.
    StateId(u64),
    /// Latest state at or before an event sequence.
    EventSeq(u64),
    /// Latest state at or before a monotonic timestamp.
    MonotonicNs(u64),
}

/// One fully reconstructed state and its index metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateInspection {
    /// State index metadata and delta provenance.
    pub record: StateRecord,
    /// Complete semantic state, not a delta.
    pub state: SemanticState,
}

/// Manifest committed with derived replay artifacts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayManifest {
    /// Artifact schema version.
    pub version: u32,
    /// Source session identifier.
    pub session_id: Uuid,
    /// Source event-log terminal hash.
    pub source_event_hash: Option<String>,
    /// Selected half-open source interval, absent for a full-session index.
    pub source_range: Option<SourceRange>,
    /// Whether completion proved the source event log immutable.
    pub source_complete: bool,
    /// Semantic states retained in the JSONL artifacts.
    pub state_selection: StateSelection,
    /// Exact Ghostty source revision.
    pub ghostty_revision: String,
    /// Number of state records retained in the artifact files.
    pub state_count: u64,
    /// Number of distinct semantic states encountered during replay.
    pub semantic_state_count: u64,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
    /// Hash of the final state retained in filtered artifact files.
    pub final_retained_state_hash: Option<String>,
    /// Relative artifact paths.
    pub files: Vec<String>,
}

/// Completed indexing paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexOutcome {
    /// Atomically committed derived directory.
    pub directory: PathBuf,
    /// Number of state records retained in artifacts.
    pub state_count: u64,
    /// Total distinct states encountered during replay.
    pub semantic_state_count: u64,
    /// Whether the source had immutable completion evidence.
    pub source_complete: bool,
    /// State retention policy used by this index.
    pub state_selection: StateSelection,
    /// Semantic state hash at the selected interval's end.
    pub final_state_hash: String,
}

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
    state_count: u64,
    row_hashes: Vec<blake3::Hash>,
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
    let artifacts = replay_with(log, None, |record, _| {
        states.push(record);
        return Ok(());
    })?;
    return Ok(CollectedReplay { states, artifacts });
}

/// Reconstruct one state selected by state, event, time, or final position.
///
/// # Errors
///
/// Returns an error when the source cannot replay or no state matches.
pub fn inspect(log: &EventLog, selector: StateSelector) -> Result<StateInspection> {
    let mut selected = None;
    replay_with(log, None, |record, state| {
        let matches = match selector {
            StateSelector::Final => true,
            StateSelector::StateId(id) => record.state_id == id,
            StateSelector::EventSeq(seq) => {
                record.event_seq.is_none_or(|event| return event <= seq)
            }
            StateSelector::MonotonicNs(time) => record.monotonic_ns <= time,
        };
        if matches {
            selected = Some(StateInspection {
                record,
                state: state.clone(),
            });
        }
        return Ok(());
    })?;
    return selected.ok_or_else(|| return invalid_replay("no semantic state matches the selector"));
}

fn replay_with(
    log: &EventLog,
    source_range: Option<SourceRange>,
    mut state_sink: impl FnMut(StateRecord, &SemanticState) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let mut builder = ReplayBuilder::new(log)?;
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
                for (offset, byte) in data.as_bytes().iter().enumerate() {
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
    return Ok(ReplayArtifacts {
        state_count: builder.state_count,
        final_state_hash,
        final_text: builder.engine.format(Format::Plain)?,
        final_vt: builder.engine.format(Format::Vt)?,
        final_html: builder.engine.format(Format::Html)?,
    });
}

impl ReplayBuilder {
    fn new(log: &EventLog) -> Result<Self> {
        let mut engine = GhosttyVt::new(log.header.cols, log.header.rows, 100_000)?;
        engine.apply_terminal_colors(&log.header.colors)?;
        return Ok(Self {
            engine,
            previous: None,
            previous_hash: None,
            state_count: 0,
            row_hashes: Vec::new(),
        });
    }

    fn capture(
        &mut self,
        event_seq: Option<u64>,
        event_byte_offset: Option<u64>,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState) -> Result<()>,
    ) -> Result<()> {
        let Some(state) = self.engine.snapshot_if_changed()? else {
            return Ok(());
        };
        return self.emit_state(state, event_seq, event_byte_offset, monotonic_ns, sink);
    }

    fn capture_baseline(
        &mut self,
        monotonic_ns: u64,
        sink: &mut impl FnMut(StateRecord, &SemanticState) -> Result<()>,
    ) -> Result<()> {
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
        sink: &mut impl FnMut(StateRecord, &SemanticState) -> Result<()>,
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
        sink(record, &state)?;
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

/// Generate replay artifacts into an atomically committed `derived` directory.
///
/// # Errors
///
/// Returns an error for invalid source logs, existing artifacts, replay
/// failures, serialization failures, or non-durable filesystem operations.
pub fn index_session(session_directory: impl AsRef<Path>) -> Result<IndexOutcome> {
    return index_session_with_options(session_directory, IndexOptions::default());
}

/// Generate replay artifacts for a validated half-open source-time interval.
///
/// # Errors
///
/// Returns an error for invalid bounds or any full-index failure.
pub fn index_session_range(
    session_directory: impl AsRef<Path>,
    start_monotonic_ns: Option<u64>,
    end_monotonic_ns: Option<u64>,
) -> Result<IndexOutcome> {
    return index_session_with_options(
        session_directory,
        IndexOptions {
            start_monotonic_ns,
            end_monotonic_ns,
            ..IndexOptions::default()
        },
    );
}

/// Generate replay artifacts under explicit completion and retention policy.
///
/// # Errors
///
/// Returns an error for incomplete sources unless `live_snapshot` is enabled,
/// invalid bounds, existing artifacts, replay failures, or artifact I/O.
pub fn index_session_with_options(
    session_directory: impl AsRef<Path>,
    options: IndexOptions,
) -> Result<IndexOutcome> {
    let session_directory = session_directory.as_ref();
    let completion_exists = session_directory.join("completion.json").is_file();
    if !completion_exists && !options.live_snapshot {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }
    let live_event_count = if !completion_exists {
        // A successful status request is a durable actor boundary: all earlier
        // buffered output is complete on disk before this process snapshots it.
        // A crashed session has no socket, so fall back to its static valid prefix.
        match request(session_directory, ControlCommand::Status) {
            Ok(ControlData::Status { next_event_seq, .. }) => Some(next_event_seq),
            Ok(_) => {
                return Err(invalid_replay(
                    "live status returned an unexpected control response",
                ));
            }
            Err(error) => {
                tracing::debug!(%error, "live snapshot has no responsive control boundary");
                None
            }
        }
    } else {
        None
    };
    let event_path = session_directory.join("events.jsonl");
    let log = match live_event_count {
        Some(event_count) => EventLog::read_prefix(&event_path, event_count)?,
        None => EventLog::read(&event_path)?,
    };
    let source_complete = if completion_exists {
        validate_completed_log(session_directory, &log)?;
        true
    } else {
        false
    };
    let final_monotonic_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    let source_range = resolve_source_range(
        options.start_monotonic_ns,
        options.end_monotonic_ns,
        final_monotonic_ns,
    )?;
    let mut directory_name = derived_directory_name_for(source_range, options.state_selection);
    if !source_complete {
        let source_hash = log.events.last().map_or_else(
            || return log.header.session_id.simple().to_string(),
            |record| {
                return record.hash.clone();
            },
        );
        let short_hash = source_hash.get(..12).unwrap_or(source_hash.as_str());
        write!(directory_name, "-snapshot-{short_hash}")
            .map_err(|_| return invalid_replay("format live snapshot directory name"))?;
    }
    let destination = session_directory.join(directory_name);
    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }
    let staging = session_directory.join(format!(".derived.{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = write_index(
        &staging,
        &log,
        source_range,
        source_complete,
        options.state_selection,
    )
    .and_then(|(artifacts, retained_state_count)| {
        commit_directory(session_directory, &staging, &destination)?;
        sync_directory(session_directory)?;
        return Ok(IndexOutcome {
            directory: destination.clone(),
            state_count: retained_state_count,
            semantic_state_count: artifacts.state_count,
            source_complete,
            state_selection: options.state_selection,
            final_state_hash: artifacts.final_state_hash,
        });
    });
    if result.is_err() && staging.exists() {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| {
            return Error::Filesystem {
                path: staging,
                source,
            };
        });
        return finish_with_cleanup(result, cleanup);
    }
    return result;
}

/// Resolve the deterministic derived artifact directory for a source range.
pub fn derived_directory_name(source_range: Option<SourceRange>) -> String {
    return derived_directory_name_for(source_range, StateSelection::All);
}

/// Resolve a deterministic derived directory for a range and retention mode.
pub fn derived_directory_name_for(
    source_range: Option<SourceRange>,
    state_selection: StateSelection,
) -> String {
    let base = match state_selection {
        StateSelection::All => DERIVED_DIRECTORY.to_owned(),
        StateSelection::Presented => format!("{DERIVED_DIRECTORY}-presented"),
    };
    return source_range.map_or_else(
        || return base.clone(),
        |range| return format!("{base}-{}", range.artifact_suffix()),
    );
}

fn write_index(
    directory: &Path,
    log: &EventLog,
    source_range: Option<SourceRange>,
    source_complete: bool,
    state_selection: StateSelection,
) -> Result<(ReplayArtifacts, u64)> {
    let states_path = directory.join("states.jsonl.zst");
    let states_file = secure_file(&states_path)?;
    let mut states = zstd::stream::write::Encoder::new(states_file, 3).map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    states.include_checksum(true).map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    let mut index = secure_writer(&directory.join("state-index.jsonl"))?;
    let mut retained_state_count = 0_u64;
    let mut final_retained_state_hash = None;
    let mut previous_retained_state = None;
    let artifacts = replay_with(log, source_range, |mut record, state| {
        if state_selection == StateSelection::Presented && !record.presented {
            return Ok(());
        }
        if state_selection == StateSelection::Presented {
            record.state_id = retained_state_count;
            record.delta = state_delta(previous_retained_state.as_ref(), state);
            previous_retained_state = Some(state.clone());
        }
        write_json_line(&mut states, &record, directory)?;
        let entry = StateIndexEntry {
            state_id: record.state_id,
            state_hash: record.state_hash.clone(),
            event_seq: record.event_seq,
            event_byte_offset: record.event_byte_offset,
            monotonic_ns: record.monotonic_ns,
            presented: record.presented,
        };
        write_json_line(&mut index, &entry, directory)?;
        final_retained_state_hash = Some(record.state_hash);
        retained_state_count = retained_state_count
            .checked_add(1)
            .ok_or_else(|| return invalid_replay("retained semantic state count overflow"))?;
        return Ok(());
    })?;
    let states_file = states.finish().map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    states_file.sync_data().map_err(|source| {
        return Error::Filesystem {
            path: states_path.clone(),
            source,
        };
    })?;
    finish_writer(index, &directory.join("state-index.jsonl"))?;
    write_secure(&directory.join("final.txt"), &artifacts.final_text)?;
    write_secure(&directory.join("final.vt"), &artifacts.final_vt)?;
    write_secure(&directory.join("final.html"), &artifacts.final_html)?;
    let manifest = ReplayManifest {
        version: STATE_FORMAT_VERSION,
        session_id: log.header.session_id,
        source_event_hash: log.events.last().map(|record| return record.hash.clone()),
        source_range,
        source_complete,
        state_selection,
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_count: retained_state_count,
        semantic_state_count: artifacts.state_count,
        final_state_hash: artifacts.final_state_hash.clone(),
        final_retained_state_hash,
        files: vec![
            "states.jsonl.zst".to_owned(),
            "state-index.jsonl".to_owned(),
            "final.txt".to_owned(),
            "final.vt".to_owned(),
            "final.html".to_owned(),
        ],
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| return json_error(directory.join("manifest.json"), source))?;
    write_secure(&directory.join("manifest.json"), &manifest_bytes)?;
    sync_directory(directory)?;
    return Ok((artifacts, retained_state_count));
}

fn state_delta(previous: Option<&SemanticState>, current: &SemanticState) -> StateDelta {
    return StateDelta {
        geometry: if previous
            .is_some_and(|state| return state.cols == current.cols && state.rows == current.rows)
        {
            None
        } else {
            Some((current.cols, current.rows))
        },
        cursor: changed(previous.map(|state| return &state.cursor), &current.cursor),
        modes: changed(previous.map(|state| return &state.modes), &current.modes),
        colors: changed(previous.map(|state| return &state.colors), &current.colors),
        active_style: changed(
            previous.map(|state| return &state.active_style),
            &current.active_style,
        ),
        metadata: changed(
            previous.map(|state| return &state.metadata),
            &current.metadata,
        ),
        rows: row_diffs(previous, current),
    };
}

fn changed<T: Clone + Eq>(previous: Option<&T>, current: &T) -> Option<T> {
    return (previous != Some(current)).then(|| return current.clone());
}

fn row_diffs(previous: Option<&SemanticState>, current: &SemanticState) -> Vec<RowDiff> {
    return current
        .grid
        .iter()
        .filter_map(|row| {
            let old_row = previous.and_then(|state| return state.grid.get(usize::from(row.row)));
            let cells = row
                .cells
                .iter()
                .filter(|cell| {
                    return old_row.and_then(|old| return old.cells.get(usize::from(cell.column)))
                        != Some(*cell);
                })
                .cloned()
                .collect::<Vec<_>>();
            return (!cells.is_empty()).then_some(RowDiff {
                row: row.row,
                cells,
            });
        })
        .collect();
}

fn cell_size(pixels: u16, cells: u16) -> u32 {
    return u32::from(pixels.checked_div(cells).unwrap_or(0).max(1));
}

fn write_json_line(writer: &mut impl Write, value: &impl Serialize, path: &Path) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| return json_error(path.to_path_buf(), source))?;
    return writer.write_all(b"\n").map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Ok(())) => return Err(primary),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn invalid_replay(reason: &str) -> Error {
    return Error::InvalidReplay {
        reason: reason.to_owned(),
    };
}
