//! Terminal presentation parsing and semantic-state tracking.

use std::{io::Write, mem::take, slice};

use uuid::Uuid;

use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource},
    terminal::state::SemanticState,
    value::ByteString,
};
use termtrace_terminal::{
    GhosttyVt, PresentationAction, PresentationTracker, PresentationTransition, QueryTracker,
};

use super::{control::MutationHistory, input::apply_input, model::TerminalGeometry};
use crate::{Error, Result};

pub(super) struct LiveTerminal {
    pub(super) semantic: GhosttyVt,
    pub(super) presented: SemanticState,
    pub(super) presentation_tracker: PresentationTracker,
    pub(super) query_tracker: QueryTracker,
    pub(super) pending_query_responses: Vec<Vec<u8>>,
    pub(super) geometry: TerminalGeometry,
    pub(super) source_event_seq: Option<u64>,
    pub(super) source_byte_offset: Option<u64>,
    pub(super) suppress_mirrored_queries: bool,
    pub(super) mirror_output: Vec<u8>,
    pub(super) mirror_sequence: Vec<u8>,
}

pub(super) fn process_semantic_output(
    live: &mut LiveTerminal,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    mutation_history: &mut MutationHistory,
    data: &[u8],
) -> Result<()> {
    for (offset, byte) in data.iter().enumerate() {
        live.source_byte_offset = Some(u64::try_from(offset).unwrap_or(u64::MAX));
        match live.presentation_tracker.push(*byte) {
            PresentationAction::Immediate => {
                process_semantic_bytes(
                    live,
                    writer,
                    event_log,
                    mutation_history,
                    slice::from_ref(byte),
                )?;
            }
            PresentationAction::Buffered => {}
            PresentationAction::Sequence { bytes, command } => {
                let before_command = if command == Some(true) {
                    Some(live.semantic.snapshot()?)
                } else {
                    None
                };
                process_semantic_bytes(live, writer, event_log, mutation_history, &bytes)?;
                if command.is_some() {
                    let after_command = live.semantic.snapshot()?;
                    match live
                        .presentation_tracker
                        .reconcile(after_command.modes.synchronized_output)
                    {
                        Some(PresentationTransition::EnterSynchronizedOutput) => {
                            if let Some(before_command) = before_command {
                                live.presented = before_command;
                            }
                        }
                        Some(PresentationTransition::ExitSynchronizedOutput) => {
                            live.presented = after_command;
                        }
                        None => {}
                    }
                }
            }
        }
    }
    return Ok(());
}

pub(super) fn process_semantic_bytes(
    live: &mut LiveTerminal,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    _mutation_history: &mut MutationHistory,
    data: &[u8],
) -> Result<()> {
    for byte in data {
        let was_tracking = live.query_tracker.is_tracking();
        let complete = live.query_tracker.push(*byte);
        live.semantic.write(slice::from_ref(byte));
        live.pending_query_responses
            .extend(live.semantic.take_responses());
        if live.suppress_mirrored_queries {
            if was_tracking || complete || live.query_tracker.is_tracking() {
                live.mirror_sequence.push(*byte);
            } else {
                live.mirror_output.push(*byte);
            }
            if complete || !live.query_tracker.is_tracking() {
                if live.pending_query_responses.is_empty() {
                    live.mirror_output.append(&mut live.mirror_sequence);
                } else {
                    // A query answered by the pinned VT must not reach the outer
                    // terminal, whose reply would otherwise be forwarded as input.
                    live.mirror_sequence.clear();
                }
            }
        }
        if complete
            || (!live.query_tracker.is_tracking() && !live.pending_query_responses.is_empty())
        {
            let query = ByteString::from(live.query_tracker.bytes());
            for response in take(&mut live.pending_query_responses) {
                let operation_id = Uuid::now_v7();
                event_log.append(Event::QueryResponse {
                    source_event_seq: live.source_event_seq,
                    source_byte_offset: live.source_byte_offset,
                    query: query.clone(),
                    response: ByteString::from(response.as_slice()),
                    responder: "pinned-libghostty-vt".to_owned(),
                })?;
                apply_input(
                    writer,
                    event_log,
                    operation_id,
                    InputSource::Host,
                    InputKind::Raw,
                    InputDescriptor::Raw,
                    None,
                    Some("terminal-query-response".to_owned()),
                    None,
                    &response,
                )?;
            }
        }
        if complete {
            live.query_tracker.clear();
        }
    }
    return Ok(());
}

pub(super) fn resize_semantic(semantic: &mut GhosttyVt, geometry: TerminalGeometry) -> Result<()> {
    semantic
        .resize(
            geometry.cols,
            geometry.rows,
            geometry_cell_size(geometry.pixel_width, geometry.cols),
            geometry_cell_size(geometry.pixel_height, geometry.rows),
        )
        .map_err(Error::from)?;
    return Ok(());
}

pub(super) fn geometry_cell_size(pixels: u16, cells: u16) -> u32 {
    return u32::from(pixels.checked_div(cells).unwrap_or(0).max(1));
}
pub(super) struct TextRegion {
    pub(super) start_column: u16,
    pub(super) end_column: u16,
    pub(super) row: u16,
    pub(super) concealed: bool,
}

pub(super) fn find_text_regions(state: &SemanticState, needle: &str) -> Vec<TextRegion> {
    let mut regions = Vec::new();
    for row in &state.grid {
        let mut text = String::new();
        let mut cells = Vec::with_capacity(row.cells.len());
        for cell in &row.cells {
            let start = text.len();
            if cell.grapheme.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&cell.grapheme);
            }
            cells.push((start, text.len(), cell.column, cell.style.invisible));
        }
        for (start, _) in text.match_indices(needle) {
            let end = start.saturating_add(needle.len());
            let mut matching = cells.iter().filter(|(cell_start, cell_end, _, _)| {
                return *cell_start < end && *cell_end > start;
            });
            let Some(first) = matching.next() else {
                continue;
            };
            let (last, concealed) = matching.fold((first, first.3), |(_, concealed), cell| {
                return (cell, concealed || cell.3);
            });
            regions.push(TextRegion {
                start_column: first.2,
                end_column: last.2.saturating_add(1),
                row: row.row,
                concealed,
            });
        }
    }
    return regions;
}
