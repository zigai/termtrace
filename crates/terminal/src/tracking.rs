//! Stateful terminal-sequence trackers used while applying semantic output.

use std::mem::take;

/// Maximum buffered presentation or query sequence.
pub const MAX_TRACKED_SEQUENCE_BYTES: usize = 64 * 1024;

/// Result of feeding one byte to synchronized-output tracking.
#[derive(Debug, Eq, PartialEq)]
pub enum PresentationAction {
    /// The byte is not part of a sequence that must be buffered.
    Immediate,
    /// The byte was buffered while a sequence remains incomplete.
    Buffered,
    /// A complete or canceled sequence is ready for terminal ingestion.
    Sequence {
        /// Exact buffered bytes.
        bytes: Vec<u8>,
        /// `Some(true)` for DECSET 2026, `Some(false)` for DECRST 2026.
        command: Option<bool>,
    },
}

/// Reconciled change in the terminal's synchronized-output mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationTransition {
    /// Synchronized output became active.
    EnterSynchronizedOutput,
    /// Synchronized output became inactive.
    ExitSynchronizedOutput,
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

/// Bounded tracker for DEC synchronized-output controls.
pub struct PresentationTracker {
    state: PresentationParseState,
    pending: Vec<u8>,
    synchronized_output: bool,
}

impl PresentationTracker {
    /// Create a tracker aligned with the terminal's current mode.
    #[must_use]
    pub fn new(synchronized_output: bool) -> Self {
        return Self {
            state: PresentationParseState::Ground,
            pending: Vec::new(),
            synchronized_output,
        };
    }

    /// Feed one byte and classify how it must be delivered to the terminal.
    pub fn push(&mut self, byte: u8) -> PresentationAction {
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
                }
                if byte == 0x1b {
                    let mut bytes = take(&mut self.pending);
                    bytes.pop();
                    self.pending.push(byte);
                    self.state = PresentationParseState::Escape;
                    return PresentationAction::Sequence {
                        bytes,
                        command: None,
                    };
                }
                parser.push(byte);
                if self.pending.len() > MAX_TRACKED_SEQUENCE_BYTES {
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

    /// Flush an incomplete buffered sequence at end of stream.
    pub fn finish(&mut self) -> Vec<u8> {
        self.state = PresentationParseState::Ground;
        return take(&mut self.pending);
    }

    /// Reconcile parsed intent with authoritative terminal mode state.
    pub fn reconcile(&mut self, synchronized_output: bool) -> Option<PresentationTransition> {
        let transition = match (self.synchronized_output, synchronized_output) {
            (false, true) => Some(PresentationTransition::EnterSynchronizedOutput),
            (true, false) => Some(PresentationTransition::ExitSynchronizedOutput),
            _ => None,
        };
        self.synchronized_output = synchronized_output;
        return transition;
    }

    fn take_sequence(&mut self, command: Option<bool>) -> PresentationAction {
        return PresentationAction::Sequence {
            bytes: take(&mut self.pending),
            command,
        };
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
                self.private = true
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

#[derive(Clone, Copy, Default)]
enum QueryKind {
    #[default]
    None,
    Escape,
    Csi,
    Osc,
    Single,
}

/// Bounded tracker for terminal query sequence boundaries.
#[derive(Default)]
pub struct QueryTracker {
    sequence: Vec<u8>,
    kind: QueryKind,
}

impl QueryTracker {
    /// Whether a query sequence is currently open.
    #[must_use]
    pub fn is_tracking(&self) -> bool {
        return !matches!(self.kind, QueryKind::None);
    }

    /// Feed one byte, returning whether the tracked query is complete.
    pub fn push(&mut self, byte: u8) -> bool {
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
                return (0x40..=0x7e).contains(&byte)
                    || self.sequence.len() > MAX_TRACKED_SEQUENCE_BYTES;
            }
            QueryKind::Osc => {
                self.sequence.push(byte);
                return byte == 0x07
                    || self.sequence.ends_with(b"\x1b\\")
                    || self.sequence.len() > MAX_TRACKED_SEQUENCE_BYTES;
            }
            QueryKind::Single => return true,
        }
    }

    /// Exact bytes accumulated for the current query.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        return &self.sequence;
    }

    /// Clear a completed query and return to ground state.
    pub fn clear(&mut self) {
        self.sequence.clear();
        self.kind = QueryKind::None;
    }
}
