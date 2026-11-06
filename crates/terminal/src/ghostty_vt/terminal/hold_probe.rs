//! Predicts the byte that begins or ends a synchronized-output hold, so a write can stop just
//! before it.
//!
//! Mirrors the escape and CSI transitions of Ghostty's parse table. A wrong prediction only
//! stops a write early, and a missed one leaves Ghostty's own hold report as the fallback.

const MAX_PARAMS: u8 = 24;
const SYNCHRONIZED_OUTPUT: u16 = 2026;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    #[default]
    Ground,
    Escape,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    Other,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct HoldProbe {
    state: State,
    intermediates: u8,
    private: bool,
    params: u8,
    accumulator: u16,
    digits: bool,
    colon: bool,
    synchronized_output: bool,
}

impl HoldProbe {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn after_escape(&mut self) {
        *self = Self {
            state: State::Escape,
            ..Self::default()
        };
    }

    pub(super) fn unknown(&mut self) {
        *self = Self {
            state: State::Other,
            ..Self::default()
        };
    }

    pub(super) fn advance(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.next(byte);
        }
    }

    pub(super) fn hold_final(&self, bytes: &[u8], held: bool) -> Option<usize> {
        let mut probe = *self;
        for (index, &byte) in bytes.iter().enumerate() {
            if probe.in_csi() && (0x40..=0x7e).contains(&byte) {
                return probe.toggles_hold(byte, held).then_some(index);
            }

            probe.next(byte);

            if probe.state == State::Ground {
                return None;
            }
        }

        return None;
    }

    fn in_csi(&self) -> bool {
        return matches!(
            self.state,
            State::CsiEntry | State::CsiParam | State::CsiIntermediate
        );
    }

    fn toggles_hold(&self, final_byte: u8, held: bool) -> bool {
        let toggles = if held { b'l' } else { b'h' };

        return (final_byte == toggles || final_byte == b'r')
            && self.intermediates == 1
            && self.private
            && !self.colon
            && self.params < MAX_PARAMS
            && (self.synchronized_output
                || (self.digits && self.accumulator == SYNCHRONIZED_OUTPUT));
    }

    fn next(&mut self, byte: u8) {
        if self.state == State::Ground {
            if byte == 0x1b {
                self.after_escape();
            }

            return;
        }

        match byte {
            0x18 | 0x1a | 0x80..=0x8f | 0x91..=0x97 | 0x99 | 0x9a | 0x9c => {
                self.reset();

                return;
            }
            0x1b => {
                self.after_escape();

                return;
            }
            0x90 | 0x98 | 0x9d..=0x9f => {
                self.unknown();

                return;
            }
            0x9b => {
                self.enter_csi();

                return;
            }
            _ => {}
        }

        match self.state {
            State::Ground | State::Other => {}
            State::Escape => match byte {
                b'[' => self.enter_csi(),
                0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => {}
                0x30..=0x4f | 0x51..=0x57 | 0x59 | 0x5a | 0x5c | 0x60..=0x7e => self.reset(),
                0xa0..=0xff => {}
                _ => self.unknown(),
            },
            State::CsiEntry => match byte {
                0x30..=0x39 | 0x3b => {
                    self.state = State::CsiParam;
                    self.param(byte);
                }
                0x3c..=0x3f => {
                    self.state = State::CsiParam;
                    self.collect(byte);
                }
                0x3a => self.state = State::CsiIgnore,
                0x20..=0x2f => {
                    self.state = State::CsiIntermediate;
                    self.collect(byte);
                }
                0x40..=0x7e => self.reset(),
                _ => {}
            },
            State::CsiParam => match byte {
                0x30..=0x3b => self.param(byte),
                0x3c..=0x3f => self.state = State::CsiIgnore,
                0x20..=0x2f => {
                    self.state = State::CsiIntermediate;
                    self.collect(byte);
                }
                0x40..=0x7e => self.reset(),
                _ => {}
            },
            State::CsiIntermediate => match byte {
                0x20..=0x2f => self.collect(byte),
                0x30..=0x3f => self.state = State::CsiIgnore,
                0x40..=0x7e => self.reset(),
                _ => {}
            },
            State::CsiIgnore => {
                if (0x40..=0x7e).contains(&byte) {
                    self.reset();
                }
            }
        }
    }

    fn enter_csi(&mut self) {
        *self = Self {
            state: State::CsiEntry,
            ..Self::default()
        };
    }

    fn collect(&mut self, byte: u8) {
        if self.intermediates == 0 {
            self.private = byte == b'?';
        }

        self.intermediates = self.intermediates.saturating_add(1);
    }

    fn param(&mut self, byte: u8) {
        if byte == b';' || byte == b':' {
            if self.params >= MAX_PARAMS {
                return;
            }

            self.synchronized_output |= self.accumulator == SYNCHRONIZED_OUTPUT;
            self.colon |= byte == b':';
            self.params = self.params.saturating_add(1);
            self.accumulator = 0;
            self.digits = false;

            return;
        }

        if self.params < MAX_PARAMS {
            self.accumulator = self
                .accumulator
                .saturating_mul(10)
                .saturating_add(u16::from(byte - b'0'));

            self.digits = true;
        }
    }
}
