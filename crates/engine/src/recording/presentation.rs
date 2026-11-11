//! Terminal presentation parsing and semantic-state tracking.

use super::{input::apply_query_response, model::TerminalGeometry};
use crate::{Error, Result};
use std::{cell::RefCell, io::Write, mem::take};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{execution::event::Event, terminal::state::SemanticState, value::ByteString};
use termtrace_terminal::{GhosttyVt, RenderHold};
use uuid::Uuid;

// Longer control sequences are never queries, so they reach the mirror without waiting.
const MAX_QUERY_SEQUENCE_BYTES: usize = 64 * 1024;

pub(super) struct LiveTerminal {
    pub(super) semantic: GhosttyVt,
    pub(super) presented: SemanticState,
    /// The VT changed since `presented` was last refreshed.
    pub(super) presented_stale: bool,
    /// Stream position: output bytes and applied resizes processed so far.
    pub(super) position: u64,
    /// Stream position of `presented`, as replay of the same session numbers it.
    pub(super) presented_position: u64,
    pub(super) geometry: TerminalGeometry,
    pub(super) source_event_seq: Option<u64>,
    pub(super) suppress_mirrored_queries: bool,
    pub(super) mirror_output: Vec<u8>,
    /// Bytes of a control sequence that has not ended, up to `MAX_QUERY_SEQUENCE_BYTES`.
    pub(super) open_sequence: Vec<u8>,
    /// The open sequence outgrew its buffer and was already mirrored.
    pub(super) open_sequence_mirrored: bool,
    pub(super) pending_query_responses: Vec<Vec<u8>>,
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

    /// Refresh `presented` from the VT unless a synchronized update is still in progress.
    pub(super) fn ensure_presented(&mut self) -> Result<()> {
        if !self.presented_stale || self.semantic.synchronized_output()? {
            return Ok(());
        }

        let latest = self.semantic.snapshot()?;
        self.replace_presented(latest);
        self.presented_stale = false;
        self.presented_position = self.position;
        return Ok(());
    }

    pub(super) fn apply_resize(&mut self, geometry: TerminalGeometry) -> Result<()> {
        resize_semantic(&mut self.semantic, geometry)?;
        self.geometry = geometry;
        self.presented_stale = true;
        self.position = self.position.saturating_add(1);
        return Ok(());
    }

    /// Mirror the bytes of a sequence the child left open when its output closed.
    pub(super) fn close_output(&mut self) {
        if self.suppress_mirrored_queries && !self.open_sequence_mirrored {
            self.mirror_output.append(&mut self.open_sequence);
        }

        self.open_sequence.clear();
        self.open_sequence_mirrored = false;
    }

    fn buffer_open_sequence(&mut self, bytes: &[u8]) {
        if !self.open_sequence_mirrored
            && self.open_sequence.len().saturating_add(bytes.len()) > MAX_QUERY_SEQUENCE_BYTES
        {
            self.open_sequence_mirrored = true;

            if self.suppress_mirrored_queries {
                self.mirror_output.extend_from_slice(&self.open_sequence);
            }
        }

        if !self.open_sequence_mirrored {
            self.open_sequence.extend_from_slice(bytes);
        } else if self.suppress_mirrored_queries {
            self.mirror_output.extend_from_slice(bytes);
        }
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
    data: &[u8],
) -> Result<()> {
    let mut offset = 0;
    while let Some(rest) = data.get(offset..)
        && !rest.is_empty()
    {
        let step = live.semantic.write_step(rest);
        let plain = rest.get(..step.plain).unwrap_or_default();
        let sequence = rest.get(step.plain..step.consumed).unwrap_or_default();
        offset = offset.saturating_add(step.consumed);
        live.position = live
            .position
            .saturating_add(u64::try_from(step.consumed).unwrap_or(u64::MAX));

        live.presented_stale = true;

        // The step stopped before the byte that begins a hold, so the screen is the kept frame.
        if step.hold_edge {
            let edge = live.semantic.snapshot()?;
            live.replace_presented(edge);
            live.presented_position = live.position;
        }

        // Ghostty's own frame stands in when the step did not stop before the hold.
        for hold in live.semantic.take_render_holds()? {
            if let RenderHold::Began(frame) = hold
                && live.presented_position != live.position.saturating_sub(1)
            {
                live.replace_presented(*frame);
                live.presented_position = live.position.saturating_sub(1);
            }
        }

        live.pending_query_responses
            .extend(live.semantic.take_responses());

        if live.suppress_mirrored_queries {
            live.mirror_output.extend_from_slice(plain);
        }

        live.buffer_open_sequence(sequence);

        if step.open {
            continue;
        }

        let responses = take(&mut live.pending_query_responses);

        // A query answered by the pinned VT must not reach the outer terminal,
        // whose reply would otherwise be forwarded as input.
        if live.suppress_mirrored_queries && !live.open_sequence_mirrored && responses.is_empty() {
            live.mirror_output.extend_from_slice(&live.open_sequence);
        }

        if !responses.is_empty() {
            let query = ByteString::from(live.open_sequence.as_slice());
            let source_byte_offset = u64::try_from(offset.saturating_sub(1)).unwrap_or(u64::MAX);

            for response in responses {
                let operation_id = Uuid::now_v7();

                event_log.append_deferred(Event::QueryResponse {
                    source_event_seq: live.source_event_seq,
                    source_byte_offset: Some(source_byte_offset),
                    query: query.clone(),
                    response: ByteString::from(response.as_slice()),
                    responder: "pinned-libghostty-vt".to_owned(),
                })?;
                apply_query_response(writer, event_log, operation_id, &response)?;
            }
        }

        live.open_sequence.clear();
        live.open_sequence_mirrored = false;
    }

    return Ok(());
}

fn resize_semantic(semantic: &mut GhosttyVt, geometry: TerminalGeometry) -> Result<()> {
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
    use crate::replay::inspect;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use termtrace_evidence::{
        artifact::create_private_directory,
        event_log::{EventLog, SessionClock},
    };
    use termtrace_model::{
        execution::event::{EventLogHeader, OperationStatus},
        terminal::replay::StateSelector,
    };
    use termtrace_terminal::{semantic_state_hash, semantic_viewport_text};

    struct Recording {
        live: LiveTerminal,
        event_log: EventLogWriter,
        child_input: Vec<u8>,
        path: PathBuf,
        _directory: TempDir,
    }

    #[derive(Debug, PartialEq)]
    struct Response {
        source_event_seq: Option<u64>,
        source_byte_offset: Option<u64>,
        query: Vec<u8>,
        response: Vec<u8>,
    }

    impl Recording {
        fn new(cols: u16, rows: u16) -> Self {
            let directory = tempfile::tempdir().expect("temporary directory");
            let session = directory.path().join("session");
            create_private_directory(&session).expect("private session");
            let path = session.join("events.jsonl");
            let clock = SessionClock::now().expect("clock");
            let header = EventLogHeader::new(
                Uuid::now_v7(),
                "0".repeat(64),
                clock.started_unix_ns(),
                cols,
                rows,
                cols * 10,
                rows * 20,
                "xterm-ghostty",
            );

            let event_log = EventLogWriter::create(&path, &header, clock).expect("event log");
            let mut semantic = GhosttyVt::new(cols, rows, 10_000).expect("terminal");
            semantic.capture_render_holds().expect("render holds");
            let presented = semantic.snapshot().expect("snapshot");
            let live = LiveTerminal {
                semantic,
                presented,
                presented_stale: false,
                position: 0,
                presented_position: 0,
                pending_query_responses: Vec::new(),
                source_event_seq: None,
                suppress_mirrored_queries: true,
                mirror_output: Vec::new(),
                open_sequence: Vec::new(),
                open_sequence_mirrored: false,
                row_hashes: RefCell::new(Vec::new()),
                cached_state_hash: RefCell::new(None),
                geometry: TerminalGeometry {
                    cols,
                    rows,
                    pixel_width: cols * 10,
                    pixel_height: rows * 20,
                },
            };

            return Self {
                live,
                event_log,
                child_input: Vec::new(),
                path,
                _directory: directory,
            };
        }

        fn read(&mut self, seq: u64, data: &[u8]) {
            self.live.source_event_seq = Some(seq);
            process_semantic_output(
                &mut self.live,
                &mut self.child_input,
                &mut self.event_log,
                data,
            )
            .expect("process output");
        }

        fn output(&mut self, data: &[u8]) {
            let seq = self
                .event_log
                .append(Event::Output {
                    data: ByteString::from(data),
                })
                .expect("output event");
            self.read(seq, data);
        }

        fn resize(&mut self, cols: u16, rows: u16) {
            let operation_id = Uuid::now_v7();
            let geometry = TerminalGeometry {
                cols,
                rows,
                pixel_width: cols * 10,
                pixel_height: rows * 20,
            };
            self.event_log
                .append(Event::ResizeIntent {
                    operation_id,
                    cols,
                    rows,
                    pixel_width: geometry.pixel_width,
                    pixel_height: geometry.pixel_height,
                })
                .expect("resize intent");

            self.live.apply_resize(geometry).expect("resize");
            self.event_log
                .append(Event::ResizeResult {
                    operation_id,
                    status: OperationStatus::Applied,
                    error_code: None,
                })
                .expect("resize result");
        }

        fn presented_text(&mut self) -> String {
            self.live
                .ensure_presented()
                .expect("refresh presented state");
            return semantic_viewport_text(&self.live.presented);
        }

        fn responses(self) -> Vec<Response> {
            drop(self.event_log);

            return read_responses(&self.path);
        }
    }

    fn read_responses(path: &Path) -> Vec<Response> {
        return EventLog::read(path)
            .expect("event log")
            .events
            .into_iter()
            .filter_map(|record| match record.event {
                Event::QueryResponse {
                    source_event_seq,
                    source_byte_offset,
                    query,
                    response,
                    ..
                } => {
                    return Some(Response {
                        source_event_seq,
                        source_byte_offset,
                        query: query.as_bytes().to_vec(),
                        response: response.as_bytes().to_vec(),
                    });
                }
                _ => return None,
            })
            .collect();
    }

    #[test]
    fn answers_back_to_back_queries_with_their_own_bytes_and_offsets() {
        let mut recording = Recording::new(20, 4);
        recording.read(7, b"ab\x1b[6n\x1b[5ncd");

        assert_eq!(recording.child_input, b"\x1b[1;3R\x1b[0n");
        assert_eq!(recording.live.mirror_output, b"abcd");
        assert_eq!(
            recording.responses(),
            vec![
                Response {
                    source_event_seq: Some(7),
                    source_byte_offset: Some(5),
                    query: b"\x1b[6n".to_vec(),
                    response: b"\x1b[1;3R".to_vec(),
                },
                Response {
                    source_event_seq: Some(7),
                    source_byte_offset: Some(9),
                    query: b"\x1b[5n".to_vec(),
                    response: b"\x1b[0n".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn attributes_a_query_split_across_reads_to_the_read_that_ends_it() {
        let mut recording = Recording::new(20, 4);
        recording.read(1, b"x\x1b[");
        assert!(recording.child_input.is_empty());
        assert_eq!(recording.live.mirror_output, b"x");

        recording.read(2, b"6");
        recording.read(3, b"ny");

        assert_eq!(recording.live.mirror_output, b"xy");
        assert_eq!(
            recording.responses(),
            vec![Response {
                source_event_seq: Some(3),
                source_byte_offset: Some(0),
                query: b"\x1b[6n".to_vec(),
                response: b"\x1b[1;2R".to_vec(),
            }]
        );
    }

    #[test]
    fn leaves_cancelled_and_c1_queries_unanswered() {
        let mut recording = Recording::new(20, 4);
        let output = b"\x1b[6\x18n\x1b[\x1a\x9b6n\xc4";
        recording.read(1, output);
        recording.read(2, b"\x9b5n");
        assert!(recording.child_input.is_empty());
        assert_eq!(
            recording.live.mirror_output,
            b"\x1b[6\x18n\x1b[\x1a\x9b6n\xc4\x9b5n"
        );
        assert_eq!(
            recording.presented_text().trim_end(),
            "n\u{fffd}6n\u{11b}5n"
        );
        assert!(recording.responses().is_empty());
    }

    #[test]
    fn answers_queries_inside_synchronized_output_without_presenting_it() {
        let mut recording = Recording::new(20, 4);
        recording.read(1, b"before");
        recording.read(2, b"\x1b[?2026h\x1b[2Jhidden\x1b[6n");

        assert_eq!(recording.child_input, b"\x1b[1;13R");
        assert_eq!(recording.presented_text().trim_end(), "before");

        recording.read(3, b"\x1b[?2026l");
        assert_eq!(recording.presented_text().trim(), "hidden");
        assert_eq!(recording.responses().len(), 1);
    }

    #[test]
    fn keeps_the_frame_from_the_start_of_a_hold_that_also_switches_screens() {
        let mut recording = Recording::new(20, 4);
        recording.read(1, b"primary\x1b[?2026;1049halternate");
        let presented = recording.live.presented.clone();
        assert!(!presented.modes.synchronized_output);
        assert!(!presented.modes.alternate_screen);
        assert_eq!(recording.presented_text().trim_end(), "primary");

        recording.read(2, b"\x1b[?2026l");
        assert_eq!(recording.presented_text().trim(), "alternate");
        assert!(recording.live.presented.modes.alternate_screen);
    }

    #[test]
    fn presents_again_when_a_reset_or_resize_ends_a_hold() {
        let mut recording = Recording::new(20, 4);
        recording.read(1, b"kept\x1b[?2026hlost");
        assert_eq!(recording.presented_text().trim_end(), "kept");

        recording.read(2, b"\x1bcafter reset");
        assert_eq!(recording.presented_text().trim_end(), "after reset");

        recording.read(3, b"\x1b[?2026h\r\x1b[Kresized");
        assert_eq!(recording.presented_text().trim_end(), "after reset");

        let geometry = TerminalGeometry {
            cols: 30,
            rows: 4,
            pixel_width: 300,
            pixel_height: 80,
        };
        recording.live.apply_resize(geometry).expect("resize");
        recording.live.presented_stale = true;
        assert_eq!(recording.presented_text().trim_end(), "resized");
    }

    #[test]
    fn live_presented_positions_match_replay_of_the_recorded_log() {
        let scenarios: [&[Option<&[u8]>]; 5] = [
            &[Some(b"hello\r\nworld")],
            &[Some(b"kept\x1b[?2026hlost")],
            &[Some(b"a\x1b[?20"), Some(b"26hb"), Some(b"c\x1b[?2026ld")],
            &[Some(b"x\x1b[?2026hy"), None, Some(b"z")],
            &[Some(b"primary\x1b[?2026;1049halternate"), None],
        ];

        for scenario in scenarios {
            let mut recording = Recording::new(20, 4);
            for step in scenario {
                match step {
                    Some(data) => recording.output(data),
                    None => recording.resize(30, 4),
                }
            }

            recording
                .live
                .ensure_presented()
                .expect("refresh presented state");
            let live_position = recording.live.presented_position;
            let live_hash = recording
                .live
                .presented_state_hash()
                .expect("presented hash");

            drop(recording.event_log);

            let log = EventLog::read(&recording.path).expect("event log");
            let replayed =
                inspect(&log, StateSelector::LatestPresented).expect("latest presented replay");
            assert_eq!(
                (live_position, live_hash),
                (replayed.record.state_id, replayed.record.state_hash),
                "{scenario:?}"
            );
        }
    }

    #[test]
    fn mirrors_long_control_strings_without_holding_them_back() {
        let mut recording = Recording::new(20, 4);
        let payload = vec![b'a'; MAX_QUERY_SEQUENCE_BYTES * 2];
        let mut output = b"\x1bP1$t".to_vec();
        output.extend_from_slice(&payload);
        recording.read(1, &output);
        assert_eq!(recording.live.mirror_output, output);
        assert!(recording.live.open_sequence.len() <= MAX_QUERY_SEQUENCE_BYTES);

        recording.read(2, b"\x1b\\done");
        output.extend_from_slice(b"\x1b\\done");
        assert_eq!(recording.live.mirror_output, output);
        assert!(recording.live.open_sequence.is_empty());
    }

    #[test]
    fn test_live_terminal_incremental_state_hash() {
        let mut semantic = GhosttyVt::new(80, 24, 10_000).expect("terminal");
        let presented = semantic.snapshot().expect("snapshot");
        let mut live = LiveTerminal {
            semantic,
            presented,
            presented_stale: false,
            position: 0,
            presented_position: 0,
            pending_query_responses: Vec::new(),
            source_event_seq: None,
            suppress_mirrored_queries: false,
            mirror_output: Vec::new(),
            open_sequence: Vec::new(),
            open_sequence_mirrored: false,
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
