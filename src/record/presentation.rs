//! Terminal presentation parsing and semantic-state tracking.

use std::{io::Write, mem::take, slice};

use uuid::Uuid;

use crate::{
    Result,
    event::{ByteString, Event, InputDescriptor, InputKind, InputSource},
    event_log::EventLogWriter,
    ghostty_vt::{GhosttyVt, SemanticState},
};

use super::{input::apply_input, model::TerminalGeometry};

pub(super) const MAX_PRESENTATION_SEQUENCE_BYTES: usize = 64 * 1024;

pub(super) struct LiveTerminal {
    pub(super) semantic: GhosttyVt,
    pub(super) presented: SemanticState,
    pub(super) presentation_tracker: PresentationTracker,
    pub(super) query_tracker: QueryTracker,
    pub(super) geometry: TerminalGeometry,
}

#[derive(Default)]
pub(super) struct QueryTracker {
    sequence: Vec<u8>,
    kind: QueryKind,
    pending_responses: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Default)]
enum QueryKind {
    #[default]
    None,
    Escape,
    Csi,
    Osc,
    Single,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresentationTransition {
    EnterSynchronizedOutput,
    ExitSynchronizedOutput,
}

pub(super) enum PresentationAction {
    Immediate,
    Buffered,
    Sequence {
        bytes: Vec<u8>,
        command: Option<bool>,
    },
}

#[derive(Clone, Copy, Default)]
enum PresentationParseState {
    #[default]
    Ground,
    Escape,
    Csi(CsiModeParser),
    CsiPassthrough(CsiModeParser),
}

#[derive(Clone, Copy, Default)]
struct CsiModeParser {
    private: bool,
    valid: bool,
    has_parameter: bool,
    parameter: u32,
    includes_synchronized_output: bool,
}

pub(super) struct PresentationTracker {
    state: PresentationParseState,
    pub(super) pending: Vec<u8>,
    synchronized_output: bool,
}
pub(super) fn process_semantic_output(
    live: &mut LiveTerminal,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    data: &[u8],
) -> Result<()> {
    for byte in data {
        match live.presentation_tracker.push(*byte) {
            PresentationAction::Immediate => {
                process_semantic_bytes(live, writer, event_log, slice::from_ref(byte))?;
            }
            PresentationAction::Buffered => {}
            PresentationAction::Sequence { bytes, command } => {
                let before_command = if command == Some(true) {
                    Some(live.semantic.snapshot()?)
                } else {
                    None
                };
                process_semantic_bytes(live, writer, event_log, &bytes)?;
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
    data: &[u8],
) -> Result<()> {
    for byte in data {
        let complete = live.query_tracker.push(*byte);
        live.semantic.write(slice::from_ref(byte));
        live.query_tracker
            .pending_responses
            .extend(live.semantic.take_responses());
        if complete
            || (!live.query_tracker.is_tracking()
                && !live.query_tracker.pending_responses.is_empty())
        {
            let query = ByteString::from(live.query_tracker.bytes());
            for response in take(&mut live.query_tracker.pending_responses) {
                event_log.append(Event::QueryResponse {
                    query: query.clone(),
                    response: ByteString::from(response.as_slice()),
                    responder: "pinned-libghostty-vt".to_owned(),
                })?;
                apply_input(
                    writer,
                    event_log,
                    Uuid::now_v7(),
                    InputSource::Host,
                    InputKind::Raw,
                    InputDescriptor::Raw,
                    Some("terminal query response".to_owned()),
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

impl PresentationTracker {
    pub(super) fn new(synchronized_output: bool) -> Self {
        return Self {
            state: PresentationParseState::Ground,
            pending: Vec::new(),
            synchronized_output,
        };
    }

    pub(super) fn push(&mut self, byte: u8) -> PresentationAction {
        match &mut self.state {
            PresentationParseState::Ground => match byte {
                0x1b => {
                    self.pending.push(byte);
                    self.state = PresentationParseState::Escape;
                    return PresentationAction::Buffered;
                }
                0x9b => {
                    self.pending.push(byte);
                    self.state = PresentationParseState::Csi(CsiModeParser::new());
                    return PresentationAction::Buffered;
                }
                _ => return PresentationAction::Immediate,
            },
            PresentationParseState::Escape => {
                if byte == 0x1b {
                    let bytes = take(&mut self.pending);
                    self.pending.push(byte);
                    return PresentationAction::Sequence {
                        bytes,
                        command: None,
                    };
                }
                self.pending.push(byte);
                if byte == b'[' {
                    self.state = PresentationParseState::Csi(CsiModeParser::new());
                    return PresentationAction::Buffered;
                }
                self.state = PresentationParseState::Ground;
                return self.take_sequence(None);
            }
            PresentationParseState::Csi(parser) => {
                self.pending.push(byte);
                if (0x40..=0x7e).contains(&byte) {
                    let command = parser.finish(byte);
                    self.state = PresentationParseState::Ground;
                    return self.take_sequence(command);
                }
                if matches!(byte, 0x18 | 0x1a) {
                    self.state = PresentationParseState::Ground;
                    return self.take_sequence(None);
                } else if byte == 0x1b {
                    let mut bytes = take(&mut self.pending);
                    bytes.pop();
                    self.pending.push(byte);
                    self.state = PresentationParseState::Escape;
                    return PresentationAction::Sequence {
                        bytes,
                        command: None,
                    };
                } else {
                    parser.push(byte);
                }
                if self.pending.len() > MAX_PRESENTATION_SEQUENCE_BYTES {
                    let parser = *parser;
                    self.state = PresentationParseState::CsiPassthrough(parser);
                    return self.take_sequence(None);
                }
                return PresentationAction::Buffered;
            }
            PresentationParseState::CsiPassthrough(parser) => {
                if (0x40..=0x7e).contains(&byte) || matches!(byte, 0x18 | 0x1a) {
                    self.state = PresentationParseState::Ground;
                } else if byte == 0x1b {
                    self.pending.push(byte);
                    self.state = PresentationParseState::Escape;
                    return PresentationAction::Buffered;
                } else {
                    parser.push(byte);
                }
                return PresentationAction::Immediate;
            }
        }
    }

    pub(super) fn finish(&mut self) -> Vec<u8> {
        self.state = PresentationParseState::Ground;
        return take(&mut self.pending);
    }

    fn take_sequence(&mut self, command: Option<bool>) -> PresentationAction {
        return PresentationAction::Sequence {
            bytes: take(&mut self.pending),
            command,
        };
    }

    fn reconcile(&mut self, synchronized_output: bool) -> Option<PresentationTransition> {
        match (self.synchronized_output, synchronized_output) {
            (false, true) => {
                self.synchronized_output = synchronized_output;
                return Some(PresentationTransition::EnterSynchronizedOutput);
            }
            (true, false) => {
                self.synchronized_output = synchronized_output;
                return Some(PresentationTransition::ExitSynchronizedOutput);
            }
            _ => {
                self.synchronized_output = synchronized_output;
                return None;
            }
        }
    }
}

impl CsiModeParser {
    fn new() -> Self {
        return Self {
            valid: true,
            ..Self::default()
        };
    }

    fn push(&mut self, byte: u8) {
        match byte {
            b'?' if !self.private && !self.has_parameter && self.parameter == 0 => {
                self.private = true;
            }
            b'0'..=b'9' if self.private && self.valid => {
                self.has_parameter = true;
                self.parameter = self
                    .parameter
                    .saturating_mul(10)
                    .saturating_add(u32::from(byte - b'0'));
            }
            b';' if self.private && self.valid => self.finish_parameter(),
            _ => self.valid = false,
        }
    }

    fn finish(mut self, final_byte: u8) -> Option<bool> {
        self.finish_parameter();
        if self.private && self.valid && self.includes_synchronized_output {
            return match final_byte {
                b'h' => Some(true),
                b'l' => Some(false),
                _ => None,
            };
        }
        return None;
    }

    fn finish_parameter(&mut self) {
        if self.has_parameter && self.parameter == 2026 {
            self.includes_synchronized_output = true;
        }
        self.has_parameter = false;
        self.parameter = 0;
    }
}

impl QueryTracker {
    fn is_tracking(&self) -> bool {
        return !matches!(self.kind, QueryKind::None);
    }

    fn push(&mut self, byte: u8) -> bool {
        const MAX_QUERY_BYTES: usize = 64 * 1024;
        match self.kind {
            QueryKind::None => match byte {
                0x1b => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Escape;
                    return false;
                }
                0x9b => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Csi;
                    return false;
                }
                0x9d => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Osc;
                    return false;
                }
                0x05 => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Single;
                    return true;
                }
                _ => return false,
            },
            QueryKind::Escape => {
                self.sequence.push(byte);
                match byte {
                    b'[' => {
                        self.kind = QueryKind::Csi;
                        return false;
                    }
                    b']' => {
                        self.kind = QueryKind::Osc;
                        return false;
                    }
                    _ => return true,
                }
            }
            QueryKind::Csi => {
                self.sequence.push(byte);
                return (0x40..=0x7e).contains(&byte) || self.sequence.len() > MAX_QUERY_BYTES;
            }
            QueryKind::Osc => {
                self.sequence.push(byte);
                return byte == 0x07
                    || self.sequence.ends_with(b"\x1b\\")
                    || self.sequence.len() > MAX_QUERY_BYTES;
            }
            QueryKind::Single => return true,
        }
    }

    fn bytes(&self) -> &[u8] {
        return &self.sequence;
    }

    fn clear(&mut self) {
        self.sequence.clear();
        self.kind = QueryKind::None;
    }
}

pub(super) fn resize_semantic(semantic: &mut GhosttyVt, geometry: TerminalGeometry) -> Result<()> {
    return semantic.resize(
        geometry.cols,
        geometry.rows,
        geometry_cell_size(geometry.pixel_width, geometry.cols),
        geometry_cell_size(geometry.pixel_height, geometry.rows),
    );
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
            let matching = cells
                .iter()
                .filter(|(cell_start, cell_end, _, _)| {
                    return *cell_start < end && *cell_end > start;
                })
                .collect::<Vec<_>>();
            let (Some(first), Some(last)) = (matching.first(), matching.last()) else {
                continue;
            };
            regions.push(TextRegion {
                start_column: first.2,
                end_column: last.2.saturating_add(1),
                row: row.row,
                concealed: matching.iter().any(|cell| return cell.3),
            });
        }
    }
    return regions;
}

pub(super) fn semantic_viewport_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        for cell in &row.cells {
            if cell.grapheme.is_empty() {
                output.push(' ');
            } else {
                output.push_str(&cell.grapheme);
            }
        }
        while output.ends_with(' ') {
            output.pop();
        }
        if index + 1 < state.grid.len() {
            output.push('\n');
        }
    }
    return output;
}
