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

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum PresentationParseState {
    #[default]
    Ground,
    Escape,
    Csi(CsiModeParser),
    CsiPassthrough,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct CsiModeParser {
    private: bool,
    valid: bool,
    parameter: Option<u32>,
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

    /// Feed a byte slice using SIMD scanning when in ground state.
    ///
    /// Returns a prefix slice of bytes that are all `PresentationAction::Immediate`,
    /// or if the prefix is empty and an action occurred on the first byte, returns that action.
    #[inline]
    pub fn push_slice<'bytes>(
        &mut self,
        bytes: &'bytes [u8],
    ) -> (&'bytes [u8], Option<PresentationAction>) {
        let Some(&first_byte) = bytes.first() else {
            return (&[], None);
        };

        if self.state == PresentationParseState::Ground {
            if let Some(pos) = memchr::memchr2(0x1b, 0x9b, bytes) {
                if pos > 0 {
                    return (bytes.get(..pos).unwrap_or(&[]), None);
                }

                let action = self.push(first_byte);

                return (&[], Some(action));
            }

            return (bytes, None);
        }

        let action = self.push(first_byte);

        return (&[], Some(action));
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
                    self.state = PresentationParseState::CsiPassthrough;

                    return self.take_sequence(None);
                }
                return PresentationAction::Buffered;
            }
            PresentationParseState::CsiPassthrough => {
                if (0x40..=0x7e).contains(&byte) || matches!(byte, 0x18 | 0x1a) {
                    self.state = PresentationParseState::Ground;
                } else if byte == 0x1b {
                    self.pending.push(byte);
                    self.state = PresentationParseState::Escape;

                    return PresentationAction::Buffered;
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
            b'?' if !self.private && self.parameter.is_none() => self.private = true,
            b'0'..=b'9' if self.private && self.valid => {
                self.parameter = Some(
                    self.parameter
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(u32::from(byte - b'0')),
                );
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
        if self.parameter == Some(2026) {
            self.includes_synchronized_output = true;
        }

        self.parameter = None;
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum QueryKind {
    #[default]
    None,
    Escape,
    Csi,
    Osc,
    String,
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
        if byte == 0x18 || byte == 0x1a {
            self.clear();

            return false;
        }

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
                0x90 | 0x98 | 0x9e | 0x9f => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::String;

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
                    b'P' | b'_' | b'^' | b'X' => {
                        self.kind = QueryKind::String;

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
            QueryKind::Osc | QueryKind::String => {
                self.sequence.push(byte);

                return byte == 0x07
                    || byte == 0x9c
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    #[test]
    fn presentation_tracker_recognizes_only_synchronized_output_mode_commands() {
        for (input, expected) in [
            (
                b"visible\x1b[?2026hhidden\x1b[?2026l".as_slice(),
                vec![true, false],
            ),
            (b"\x1b[?25;2026h".as_slice(), vec![true]),
            (b"\x1b[?;2026;h".as_slice(), vec![true]),
            (b"\x1b[?0;2026h".as_slice(), vec![true]),
            (b"\x1b[?2026;0h".as_slice(), vec![true]),
            (b"\x1b[?h".as_slice(), Vec::new()),
            (b"\x1b[?0h".as_slice(), Vec::new()),
            (b"\x1b[?999999999999999999999h".as_slice(), Vec::new()),
            (b"\x9b?2026l".as_slice(), vec![false]),
            (b"\x1b[2026h".as_slice(), Vec::new()),
            (b"\x1b[?20260h".as_slice(), Vec::new()),
            (b"\x1b[?2026:1h".as_slice(), Vec::new()),
            (b"\x1b[?2026\x18h".as_slice(), Vec::new()),
        ] {
            let mut tracker = PresentationTracker::new(false);
            let mut commands = Vec::new();
            let mut framed = Vec::new();

            for byte in input {
                match tracker.push(*byte) {
                    PresentationAction::Immediate => framed.push(*byte),
                    PresentationAction::Buffered => {}
                    PresentationAction::Sequence { bytes, command } => {
                        framed.extend(bytes);

                        if let Some(command) = command {
                            commands.push(command);
                        }
                    }
                }
            }

            framed.extend(tracker.finish());

            assert_eq!(commands, expected, "input: {input:?}");
            assert_eq!(framed, input, "framing must be lossless");
        }

        const TRACKED_SEQUENCE_BOUND: usize = 64 * 1024;

        let mut tracker = PresentationTracker::new(false);

        let mut longest_sequence = 0;
        for byte in b"\x1b[?"
            .iter()
            .copied()
            .chain(iter::repeat_n(b'0', TRACKED_SEQUENCE_BOUND * 2))
        {
            if let PresentationAction::Sequence { bytes, .. } = tracker.push(byte) {
                longest_sequence = longest_sequence.max(bytes.len());
            }
        }

        longest_sequence = longest_sequence.max(tracker.finish().len());

        assert!(longest_sequence <= TRACKED_SEQUENCE_BOUND + 1);
    }

    #[test]
    fn query_tracker_tracks_dcs_and_handles_st_and_cancellation() {
        let mut tracker = QueryTracker::default();
        // DCS sequence ESC P $ q " p ESC \
        for byte in b"\x1bP$q\"p" {
            assert!(!tracker.push(*byte));
            assert!(tracker.is_tracking());
        }

        assert!(!tracker.push(0x1b));
        assert!(tracker.push(b'\\'));
        assert_eq!(tracker.bytes(), b"\x1bP$q\"p\x1b\\");

        tracker.clear();

        // CAN cancels tracking
        assert!(!tracker.push(0x1b));
        assert!(!tracker.push(b'['));
        assert!(!tracker.push(0x18));
        assert!(!tracker.is_tracking());
        assert!(tracker.bytes().is_empty());

        // 8-bit ST completes OSC
        assert!(!tracker.push(0x9d));
        assert!(!tracker.push(b'0'));
        assert!(tracker.push(0x9c));
        assert_eq!(tracker.bytes(), &[0x9d, b'0', 0x9c]);
    }

    #[test]
    fn presentation_tracker_push_slice_matches_byte_push_exactly() {
        let test_inputs = [
            b"simple plain text without escapes".as_slice(),
            b"visible\x1b[?2026hhidden\x1b[?2026l".as_slice(),
            b"\x1b[?25;2026h\x1b[?2026lremaining text".as_slice(),
            b"\x9b?2026ltext after 8-bit csi".as_slice(),
            b"prefix\x1b[?2026h\x1b[?2026l\x9b?2026hsuffix".as_slice(),
            b"\x1b[?2026h".as_slice(),
            b"\x1b".as_slice(),
        ];

        for input in test_inputs {
            let mut byte_tracker = PresentationTracker::new(false);
            let mut byte_commands = Vec::new();
            let mut byte_framed = Vec::new();

            for byte in input {
                match byte_tracker.push(*byte) {
                    PresentationAction::Immediate => byte_framed.push(*byte),
                    PresentationAction::Buffered => {}
                    PresentationAction::Sequence { bytes, command } => {
                        byte_framed.extend(bytes);

                        if let Some(cmd) = command {
                            byte_commands.push(cmd);
                        }
                    }
                }
            }

            byte_framed.extend(byte_tracker.finish());

            let mut slice_tracker = PresentationTracker::new(false);
            let mut slice_commands = Vec::new();
            let mut slice_framed = Vec::new();

            let mut offset = 0;
            while offset < input.len() {
                let remaining = input.get(offset..).unwrap_or(&[]);
                let (immediate, action) = slice_tracker.push_slice(remaining);

                if !immediate.is_empty() {
                    slice_framed.extend_from_slice(immediate);
                    offset += immediate.len();
                    continue;
                }

                let Some(action) = action else {
                    break;
                };

                offset += 1;

                match action {
                    PresentationAction::Immediate => {
                        if let Some(&b) = input.get(offset - 1) {
                            slice_framed.push(b);
                        }
                    }
                    PresentationAction::Buffered => {}
                    PresentationAction::Sequence { bytes, command } => {
                        slice_framed.extend(bytes);

                        if let Some(cmd) = command {
                            slice_commands.push(cmd);
                        }
                    }
                }
            }

            slice_framed.extend(slice_tracker.finish());
            assert_eq!(
                byte_framed, slice_framed,
                "framed bytes must match for input {input:?}"
            );
            assert_eq!(
                byte_commands, slice_commands,
                "commands must match for input {input:?}"
            );
        }
    }
}
