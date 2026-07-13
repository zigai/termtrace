//! Deterministic byte-granular replay and semantic state indexing.

use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        commit_directory, create_private_directory, finish_writer, secure_writer, sync_directory,
        write_secure,
    },
    event::{Event, OperationStatus},
    event_log::EventLog,
    ghostty_vt::{
        CellState, CellStyle, ColorState, CursorState, DirtyState, Format, GhosttyVt, ModeState,
        SemanticState, TerminalMetadata,
    },
};

/// Pinned Ghostty source revision used by the semantic adapter.
pub const GHOSTTY_REVISION: &str = "55a3e33ab26a23d75b274b23c7f76d837db00578";
/// Derived artifact format version.
pub const STATE_FORMAT_VERSION: u32 = 1;
/// Relative directory containing replay-derived artifacts.
pub const DERIVED_DIRECTORY: &str = "derived";

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
    /// Canonical event responsible for this state; absent for initial state.
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
    /// Final semantic state hash.
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
    /// Exact Ghostty source revision.
    pub ghostty_revision: String,
    /// Number of distinct semantic states.
    pub state_count: u64,
    /// Final semantic state hash.
    pub final_state_hash: String,
    /// Relative artifact paths.
    pub files: Vec<String>,
}

/// Completed indexing paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexOutcome {
    /// Atomically committed derived directory.
    pub directory: PathBuf,
    /// Number of distinct states.
    pub state_count: u64,
    /// Final semantic state hash.
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
struct StateHashMetadata<'a> {
    cols: u16,
    rows: u16,
    cursor: &'a CursorState,
    modes: &'a ModeState,
    colors: &'a ColorState,
    active_style: &'a CellStyle,
    metadata: &'a TerminalMetadata,
}

/// Replay a validated event log and collect every distinct state.
///
/// # Errors
///
/// Returns an error when libghostty-vt or state serialization fails.
pub fn replay(log: &EventLog) -> Result<CollectedReplay> {
    let mut states = Vec::new();
    let artifacts = replay_with(log, |record, _| {
        states.push(record);
        Ok(())
    })?;
    Ok(CollectedReplay { states, artifacts })
}

/// Reconstruct one state selected by state, event, time, or final position.
///
/// # Errors
///
/// Returns an error when the source cannot replay or no state matches.
pub fn inspect(log: &EventLog, selector: StateSelector) -> Result<StateInspection> {
    let mut selected = None;
    replay_with(log, |record, state| {
        let matches = match selector {
            StateSelector::Final => true,
            StateSelector::StateId(id) => record.state_id == id,
            StateSelector::EventSeq(seq) => record.event_seq.is_none_or(|event| event <= seq),
            StateSelector::MonotonicNs(time) => record.monotonic_ns <= time,
        };
        if matches {
            selected = Some(StateInspection {
                record,
                state: state.clone(),
            });
        }
        Ok(())
    })?;
    selected.ok_or_else(|| invalid_replay("no semantic state matches the selector"))
}

fn replay_with(
    log: &EventLog,
    mut state_sink: impl FnMut(StateRecord, &SemanticState) -> Result<()>,
) -> Result<ReplayArtifacts> {
    let mut builder = ReplayBuilder::new(log)?;
    builder.capture(None, None, 0, &mut state_sink)?;
    let mut resize_intents = HashMap::new();
    for record in &log.events {
        match &record.event {
            Event::Output { data } => {
                for (offset, byte) in data.as_bytes().iter().enumerate() {
                    builder.engine.write(std::slice::from_ref(byte));
                    drop(builder.engine.take_responses());
                    builder.capture(
                        Some(record.seq),
                        Some(u64::try_from(offset).unwrap_or(u64::MAX)),
                        record.monotonic_ns,
                        &mut state_sink,
                    )?;
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
                let geometry = resize_intents
                    .remove(operation_id)
                    .ok_or_else(|| invalid_replay("applied resize result is missing its intent"))?;
                builder.engine.resize(
                    geometry.cols,
                    geometry.rows,
                    cell_size(geometry.pixel_width, geometry.cols),
                    cell_size(geometry.pixel_height, geometry.rows),
                )?;
                builder.capture(Some(record.seq), None, record.monotonic_ns, &mut state_sink)?;
            }
            Event::ResizeResult { operation_id, .. } => {
                resize_intents.remove(operation_id);
            }
            _ => {}
        }
    }
    let final_state_hash = builder
        .previous_hash
        .clone()
        .ok_or_else(|| invalid_replay("replay produced no initial state"))?;
    Ok(ReplayArtifacts {
        state_count: builder.state_count,
        final_state_hash,
        final_text: builder.engine.format(Format::Plain)?,
        final_vt: builder.engine.format(Format::Vt)?,
        final_html: builder.engine.format(Format::Html)?,
    })
}

impl ReplayBuilder {
    fn new(log: &EventLog) -> Result<Self> {
        let mut engine = GhosttyVt::new(log.header.cols, log.header.rows, 100_000)?;
        engine.apply_terminal_colors(&log.header.colors)?;
        Ok(Self {
            engine,
            previous: None,
            previous_hash: None,
            state_count: 0,
            row_hashes: Vec::new(),
        })
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
            .ok_or_else(|| invalid_replay("semantic state identifier overflow"))?;
        self.previous = Some(state);
        self.previous_hash = Some(hash);
        Ok(())
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
                    .and_then(|previous| previous.grid.get(usize::from(row.row)))
                    .is_none_or(|previous| previous.cells != row.cells);
            if changed {
                let encoded = serde_json::to_vec(&row.cells)
                    .map_err(|source| json_error(PathBuf::from("<semantic-row>"), source))?;
                self.row_hashes[usize::from(row.row)] = blake3::hash(&encoded);
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
            .map_err(|source| json_error(PathBuf::from("<semantic-metadata>"), source))?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"termtrace-semantic-state-v1\0");
        hash.update(&encoded);
        for row_hash in &self.row_hashes {
            hash.update(row_hash.as_bytes());
        }
        Ok(hash.finalize().to_hex().to_string())
    }
}

/// Generate replay artifacts into an atomically committed `derived` directory.
///
/// # Errors
///
/// Returns an error for invalid source logs, existing artifacts, replay
/// failures, serialization failures, or non-durable filesystem operations.
pub fn index_session(session_directory: impl AsRef<Path>) -> Result<IndexOutcome> {
    let session_directory = session_directory.as_ref();
    let event_path = session_directory.join("events.jsonl");
    let log = EventLog::read(&event_path)?;
    let destination = session_directory.join(DERIVED_DIRECTORY);
    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }
    let staging = session_directory.join(format!(".derived.{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = write_index(&staging, &log).and_then(|artifacts| {
        commit_directory(session_directory, &staging, &destination)?;
        sync_directory(session_directory)?;
        Ok(IndexOutcome {
            directory: destination.clone(),
            state_count: artifacts.state_count,
            final_state_hash: artifacts.final_state_hash,
        })
    });
    if result.is_err() && staging.exists() {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| Error::Filesystem {
            path: staging,
            source,
        });
        return finish_with_cleanup(result, cleanup);
    }
    result
}

fn write_index(directory: &Path, log: &EventLog) -> Result<ReplayArtifacts> {
    let mut states = secure_writer(&directory.join("states.jsonl"))?;
    let mut index = secure_writer(&directory.join("state-index.jsonl"))?;
    let artifacts = replay_with(log, |record, _| {
        write_json_line(&mut states, &record, directory)?;
        let entry = StateIndexEntry {
            state_id: record.state_id,
            state_hash: record.state_hash.clone(),
            event_seq: record.event_seq,
            event_byte_offset: record.event_byte_offset,
            monotonic_ns: record.monotonic_ns,
            presented: record.presented,
        };
        write_json_line(&mut index, &entry, directory)
    })?;
    finish_writer(states, &directory.join("states.jsonl"))?;
    finish_writer(index, &directory.join("state-index.jsonl"))?;
    write_secure(&directory.join("final.txt"), &artifacts.final_text)?;
    write_secure(&directory.join("final.vt"), &artifacts.final_vt)?;
    write_secure(&directory.join("final.html"), &artifacts.final_html)?;
    let manifest = ReplayManifest {
        version: STATE_FORMAT_VERSION,
        session_id: log.header.session_id,
        source_event_hash: log.events.last().map(|record| record.hash.clone()),
        ghostty_revision: GHOSTTY_REVISION.to_owned(),
        state_count: artifacts.state_count,
        final_state_hash: artifacts.final_state_hash.clone(),
        files: vec![
            "states.jsonl".to_owned(),
            "state-index.jsonl".to_owned(),
            "final.txt".to_owned(),
            "final.vt".to_owned(),
            "final.html".to_owned(),
        ],
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| json_error(directory.join("manifest.json"), source))?;
    write_secure(&directory.join("manifest.json"), &manifest_bytes)?;
    sync_directory(directory)?;
    Ok(artifacts)
}

fn state_delta(previous: Option<&SemanticState>, current: &SemanticState) -> StateDelta {
    StateDelta {
        geometry: if previous
            .is_some_and(|state| state.cols == current.cols && state.rows == current.rows)
        {
            None
        } else {
            Some((current.cols, current.rows))
        },
        cursor: changed(previous.map(|state| &state.cursor), &current.cursor),
        modes: changed(previous.map(|state| &state.modes), &current.modes),
        colors: changed(previous.map(|state| &state.colors), &current.colors),
        active_style: changed(
            previous.map(|state| &state.active_style),
            &current.active_style,
        ),
        metadata: changed(previous.map(|state| &state.metadata), &current.metadata),
        rows: row_diffs(previous, current),
    }
}

fn changed<T: Clone + Eq>(previous: Option<&T>, current: &T) -> Option<T> {
    (previous != Some(current)).then(|| current.clone())
}

fn row_diffs(previous: Option<&SemanticState>, current: &SemanticState) -> Vec<RowDiff> {
    current
        .grid
        .iter()
        .filter_map(|row| {
            let old_row = previous.and_then(|state| state.grid.get(usize::from(row.row)));
            let cells = row
                .cells
                .iter()
                .filter(|cell| {
                    old_row.and_then(|old| old.cells.get(usize::from(cell.column))) != Some(*cell)
                })
                .cloned()
                .collect::<Vec<_>>();
            (!cells.is_empty()).then_some(RowDiff {
                row: row.row,
                cells,
            })
        })
        .collect()
}

fn cell_size(pixels: u16, cells: u16) -> u32 {
    u32::from(pixels.checked_div(cells).unwrap_or(0).max(1))
}

fn write_json_line(
    writer: &mut BufWriter<File>,
    value: &impl Serialize,
    path: &Path,
) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| json_error(path.to_path_buf(), source))?;
    writer.write_all(b"\n").map_err(|source| Error::Filesystem {
        path: path.to_path_buf(),
        source,
    })
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(Error::CleanupFailed {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    }
}

fn invalid_replay(reason: &str) -> Error {
    Error::InvalidReplay {
        reason: reason.to_owned(),
    }
}
