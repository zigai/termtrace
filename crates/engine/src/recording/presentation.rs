//! Terminal presentation parsing and semantic-state tracking.

use super::{control::MutationHistory, input::apply_query_response, model::TerminalGeometry};
use crate::{Error, Result};
use std::{cell::RefCell, io::Write, mem::take, slice};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{execution::event::Event, terminal::state::SemanticState, value::ByteString};
use termtrace_terminal::{
    GhosttyVt, PresentationAction, PresentationTracker, PresentationTransition, QueryTracker,
};
use uuid::Uuid;

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
    pub(super) row_hashes: RefCell<Vec<Option<blake3::Hash>>>,
    pub(super) cached_state_hash: RefCell<Option<String>>,
}

impl LiveTerminal {
    pub(super) fn presented_state_hash(&self) -> Result<String> {
        if let Some(hash) = self.cached_state_hash.borrow().as_ref() {
            return Ok(hash.clone());
        }

        let mut row_hashes = self.row_hashes.borrow_mut();
        row_hashes.resize(self.presented.grid.len(), None);
        let hashes = self
            .presented
            .grid
            .iter()
            .zip(row_hashes.iter_mut())
            .map(|(row, cached)| {
                let hash = match *cached {
                    Some(hash) => hash,
                    None => termtrace_terminal::hash_semantic_row(row)
                        .map_err(crate::Error::ScreenFrame)?,
                };
                *cached = Some(hash);

                return Ok(hash);
            })
            .collect::<Result<Vec<_>>>()?;

        let hash =
            termtrace_terminal::semantic_state_hash_from_row_hashes(&self.presented, &hashes)
                .map_err(crate::Error::ScreenFrame)?;
        *self.cached_state_hash.borrow_mut() = Some(hash.clone());

        return Ok(hash);
    }

    pub(super) fn replace_presented(&mut self, presented: SemanticState) {
        let row_hashes = self.row_hashes.get_mut();
        row_hashes.resize(presented.grid.len(), None);

        // Dirty flags refer to the last Ghostty snapshot, which may not be the
        // last presented or hashed snapshot (for example during synchronized output).
        for (index, cached) in row_hashes.iter_mut().enumerate() {
            if self.presented.grid.get(index) != presented.grid.get(index) {
                *cached = None;
            }
        }

        self.presented = presented;
        *self.cached_state_hash.get_mut() = None;
    }
}

pub(super) fn process_semantic_output(
    live: &mut LiveTerminal,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    mutation_history: &mut MutationHistory,
    data: &[u8],
) -> Result<()> {
    let mut immediate_start: Option<usize> = None;

    let mut offset = 0;
    while offset < data.len() {
        let remaining = data.get(offset..).unwrap_or(&[]);
        let (immediate, action) = live.presentation_tracker.push_slice(remaining);

        if !immediate.is_empty() {
            if immediate_start.is_none() {
                immediate_start = Some(offset);
            }

            offset += immediate.len();
            continue;
        }

        let Some(action) = action else {
            break;
        };

        let current_offset = offset;
        offset += 1;

        match action {
            PresentationAction::Immediate => {
                if immediate_start.is_none() {
                    immediate_start = Some(current_offset);
                }
            }
            PresentationAction::Buffered => {
                if let Some(start) = immediate_start.take() {
                    let end = current_offset.max(start);
                    live.source_byte_offset = Some(u64::try_from(start).unwrap_or(u64::MAX));
                    process_semantic_bytes(
                        live,
                        writer,
                        event_log,
                        mutation_history,
                        data.get(start..end).unwrap_or(&[]),
                    )?;
                }

                live.source_byte_offset = Some(u64::try_from(current_offset).unwrap_or(u64::MAX));
            }
            PresentationAction::Sequence { bytes, command } => {
                let seq_start = current_offset.saturating_add(1).saturating_sub(bytes.len());
                if let Some(start) = immediate_start.take() {
                    let end = seq_start.max(start);
                    live.source_byte_offset = Some(u64::try_from(start).unwrap_or(u64::MAX));
                    process_semantic_bytes(
                        live,
                        writer,
                        event_log,
                        mutation_history,
                        data.get(start..end).unwrap_or(&[]),
                    )?;
                }

                live.source_byte_offset = Some(u64::try_from(current_offset).unwrap_or(u64::MAX));

                let before_command =
                    if command == Some(true) { Some(live.semantic.snapshot()?) } else { None };
                process_semantic_bytes(live, writer, event_log, mutation_history, &bytes)?;

                if command.is_some() {
                    let after_command = live.semantic.snapshot()?;

                    match live
                        .presentation_tracker
                        .reconcile(after_command.modes.synchronized_output)
                    {
                        Some(PresentationTransition::EnterSynchronizedOutput) => {
                            if let Some(before_command) = before_command {
                                live.replace_presented(before_command);
                            }
                        }
                        Some(PresentationTransition::ExitSynchronizedOutput) => {
                            live.replace_presented(after_command);
                        }
                        None => {}
                    }
                }
            }
        }
    }

    if let Some(start) = immediate_start.take() {
        live.source_byte_offset = Some(u64::try_from(start).unwrap_or(u64::MAX));
        process_semantic_bytes(
            live,
            writer,
            event_log,
            mutation_history,
            data.get(start..).unwrap_or(&[]),
        )?;
    }

    return Ok(());
}

fn is_query_start_byte(byte: u8) -> bool {
    return matches!(byte, 0x05 | 0x1b | 0x90 | 0x98 | 0x9b | 0x9d | 0x9e | 0x9f);
}

pub(super) fn process_semantic_bytes(
    live: &mut LiveTerminal,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    _mutation_history: &mut MutationHistory,
    data: &[u8],
) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let is_sequence = matches!(data.first(), Some(0x1b | 0x9b));
    let base_offset = live.source_byte_offset;

    let mut index = 0;
    while index < data.len() {
        if !live.query_tracker.is_tracking() && live.pending_query_responses.is_empty() {
            let start = index;
            while index < data.len()
                && data
                    .get(index)
                    .is_some_and(|&b| return !is_query_start_byte(b))
            {
                index += 1;
            }

            if index > start {
                let chunk = data.get(start..index).unwrap_or(&[]);
                live.query_tracker.observe_plain_bytes(chunk);
                live.semantic.write(chunk);

                if live.suppress_mirrored_queries {
                    live.mirror_output.extend_from_slice(chunk);
                }

                continue;
            }
        }

        let Some(&byte) = data.get(index) else {
            break;
        };

        if !is_sequence {
            let offset_delta = u64::try_from(index).unwrap_or(u64::MAX);
            live.source_byte_offset =
                base_offset.map(|base| return base.saturating_add(offset_delta));
        }

        index += 1;

        let was_tracking = live.query_tracker.is_tracking();
        let complete = live.query_tracker.push(byte);
        live.semantic.write(slice::from_ref(&byte));

        if was_tracking
            || complete
            || live.query_tracker.is_tracking()
            || !live.pending_query_responses.is_empty()
        {
            live.pending_query_responses
                .extend(live.semantic.take_responses());
        }

        if live.suppress_mirrored_queries {
            if was_tracking || complete || live.query_tracker.is_tracking() {
                live.mirror_sequence.push(byte);
            } else {
                live.mirror_output.push(byte);
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

                event_log.append_deferred(Event::QueryResponse {
                    source_event_seq: live.source_event_seq,
                    source_byte_offset: live.source_byte_offset,
                    query: query.clone(),
                    response: ByteString::from(response.as_slice()),
                    responder: "pinned-libghostty-vt".to_owned(),
                })?;
                apply_query_response(writer, event_log, operation_id, &response)?;
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

        for cell in row.cells.iter() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_terminal::semantic_state_hash;

    #[test]
    fn test_live_terminal_incremental_state_hash() {
        let mut semantic = GhosttyVt::new(80, 24, 10_000).expect("terminal");
        let presented = semantic.snapshot().expect("snapshot");
        let mut live = LiveTerminal {
            semantic,
            presentation_tracker: PresentationTracker::new(presented.modes.synchronized_output),
            presented,
            query_tracker: QueryTracker::default(),
            pending_query_responses: Vec::new(),
            source_event_seq: None,
            source_byte_offset: None,
            suppress_mirrored_queries: false,
            mirror_output: Vec::new(),
            mirror_sequence: Vec::new(),
            row_hashes: RefCell::new(Vec::new()),
            cached_state_hash: RefCell::new(None),
            geometry: TerminalGeometry {
                cols: 80,
                rows: 24,
                pixel_width: 800,
                pixel_height: 480,
            },
        };

        assert!(live.cached_state_hash.borrow().is_none());
        assert!(live.row_hashes.borrow().is_empty());

        let expected_hash = semantic_state_hash(&live.presented).expect("baseline hash");
        let computed_hash = live.presented_state_hash().expect("computed hash");
        assert_eq!(computed_hash, expected_hash);
        assert_eq!(
            live.cached_state_hash.borrow().as_deref(),
            Some(expected_hash.as_str())
        );
        assert_eq!(live.row_hashes.borrow().len(), live.presented.grid.len());

        let cached_hash = live.presented_state_hash().expect("cached hash");
        assert_eq!(cached_hash, expected_hash);

        live.semantic.write(b"Hello incremental hash!\r\n");
        let updated = live.semantic.snapshot().expect("updated snapshot");
        live.replace_presented(updated);
        let updated_expected = semantic_state_hash(&live.presented).expect("updated expected");
        let updated_computed = live.presented_state_hash().expect("updated computed");
        assert_eq!(updated_computed, updated_expected);
        assert_ne!(updated_computed, expected_hash);

        // An intermediate consumer can clear dirty flags before presentation.
        live.semantic.write(b"\rCHANGED\r");
        let _intermediate = live.semantic.snapshot().expect("intermediate snapshot");
        let clean = live.semantic.snapshot().expect("clean snapshot");
        assert!(clean.grid.iter().all(|row| return !row.dirty));

        live.replace_presented(clean);

        assert_eq!(
            live.presented_state_hash().expect("clean snapshot hash"),
            semantic_state_hash(&live.presented).expect("full clean snapshot hash")
        );

        // Dirty rows from several replacements must accumulate until hashed.
        for output in [b"\x1b[1;1Hfirst".as_slice(), b"\x1b[2;1Hsecond".as_slice()] {
            live.semantic.write(output);
            let updated = live.semantic.snapshot().expect("pending snapshot");
            live.replace_presented(updated);
        }

        assert_eq!(
            live.presented_state_hash().expect("combined snapshot hash"),
            semantic_state_hash(&live.presented).expect("full combined snapshot hash")
        );
    }
}
