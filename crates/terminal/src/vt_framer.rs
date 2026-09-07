//! Lossless streaming framing of VT output bytes.

use std::mem;

use termtrace_model::execution::event::{Event, EventRecord};
pub use termtrace_model::terminal::vt::{VtSourceSegment, VtToken, VtTokenKind};
use termtrace_model::value::ByteString;

use crate::{Error, Result};
const DEFAULT_MAX_TOKEN_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FramerState {
    Ground,
    Escape,
    Csi,
    String { kind: VtTokenKind, osc: bool },
    StringEscape { kind: VtTokenKind, osc: bool },
}

#[derive(Clone, Debug)]
struct TaggedByte {
    value: u8,
    event_seq: u64,
    byte_offset: u64,
    monotonic_ns: u64,
}

/// Bounded stateful VT framer that accepts successive canonical output reads.
#[derive(Debug)]
pub struct VtFramer {
    state: FramerState,
    token: Vec<u8>,
    source: Vec<VtSourceSegment>,
    printable_utf8_remaining: u8,
    next_token_id: u64,
    max_token_bytes: usize,
}

impl Default for VtFramer {
    fn default() -> Self {
        return Self::new(DEFAULT_MAX_TOKEN_BYTES);
    }
}

impl VtFramer {
    /// Construct a framer with an explicit maximum buffered token size.
    #[must_use]
    pub fn new(max_token_bytes: usize) -> Self {
        return Self {
            state: FramerState::Ground,
            token: Vec::new(),
            source: Vec::new(),
            printable_utf8_remaining: 0,
            next_token_id: 0,
            max_token_bytes,
        };
    }

    /// Feed one canonical output-event payload.
    ///
    /// # Errors
    ///
    /// Returns an error when one token exceeds the configured memory bound.
    pub fn feed(
        &mut self,
        event_seq: u64,
        monotonic_ns: u64,
        bytes: &[u8],
    ) -> Result<Vec<VtToken>> {
        let mut emitted = Vec::new();
        for (offset, value) in bytes.iter().copied().enumerate() {
            let tagged = TaggedByte {
                value,
                event_seq,
                byte_offset: u64::try_from(offset).unwrap_or(u64::MAX),
                monotonic_ns,
            };
            self.consume(tagged, &mut emitted)?;
        }
        return Ok(emitted);
    }

    /// Feed one byte with its exact position in a canonical output event.
    ///
    /// # Errors
    ///
    /// Returns an error when the byte makes the current token exceed the configured bound.
    pub fn feed_byte_at(
        &mut self,
        event_seq: u64,
        monotonic_ns: u64,
        byte_offset: u64,
        value: u8,
    ) -> Result<Vec<VtToken>> {
        let mut emitted = Vec::new();
        self.consume(
            TaggedByte {
                value,
                event_seq,
                byte_offset,
                monotonic_ns,
            },
            &mut emitted,
        )?;
        return Ok(emitted);
    }

    /// Finish the stream, emitting printable bytes and labeling open controls incomplete.
    ///
    /// # Errors
    ///
    /// Returns an error only if the configured token size is zero and data was buffered.
    pub fn finish(mut self) -> Result<Vec<VtToken>> {
        let mut emitted = Vec::new();
        if !self.token.is_empty() {
            let kind = if self.state == FramerState::Ground {
                VtTokenKind::Printable
            } else {
                VtTokenKind::Incomplete
            };
            emitted.push(self.emit(kind));
        }
        return Ok(emitted);
    }

    fn consume(&mut self, byte: TaggedByte, emitted: &mut Vec<VtToken>) -> Result<()> {
        match self.state {
            FramerState::Ground => return self.consume_ground(byte, emitted),
            FramerState::Escape => return self.consume_escape(byte, emitted),
            FramerState::Csi => return self.consume_csi(byte, emitted),
            FramerState::String { kind, osc } => {
                return self.consume_string(byte, kind, osc, emitted);
            }
            FramerState::StringEscape { kind, osc } => {
                let value = byte.value;
                self.push(byte)?;
                if value == b'\\' {
                    emitted.push(self.emit(kind));
                    self.state = FramerState::Ground;
                } else {
                    self.state = FramerState::String { kind, osc };
                }
                return Ok(());
            }
        }
    }

    fn consume_ground(&mut self, byte: TaggedByte, emitted: &mut Vec<VtToken>) -> Result<()> {
        if self.printable_utf8_remaining > 0 {
            if (0x80..=0xbf).contains(&byte.value) {
                self.printable_utf8_remaining -= 1;
                return self.push(byte);
            }
            self.printable_utf8_remaining = 0;
            self.flush_printable(emitted);
            return self.consume_ground(byte, emitted);
        }
        match byte.value {
            0x1b => {
                self.flush_printable(emitted);
                self.push(byte)?;
                self.state = FramerState::Escape;
            }
            0x9b => self.begin_c1_sequence(byte, VtTokenKind::Csi, emitted)?,
            0x9d => self.begin_string(byte, VtTokenKind::Osc, true, emitted)?,
            0x90 => self.begin_string(byte, VtTokenKind::Dcs, false, emitted)?,
            0x9f => self.begin_string(byte, VtTokenKind::Apc, false, emitted)?,
            0x00..=0x1f | 0x7f => {
                self.flush_printable(emitted);
                self.push(byte)?;
                emitted.push(self.emit(VtTokenKind::C0));
            }
            0x80..=0x9f => {
                self.flush_printable(emitted);
                self.push(byte)?;
                emitted.push(self.emit(VtTokenKind::C1));
            }
            value => {
                self.printable_utf8_remaining = match value {
                    0xc2..=0xdf => 1,
                    0xe0..=0xef => 2,
                    0xf0..=0xf4 => 3,
                    _ => 0,
                };
                self.push(byte)?;
            }
        }
        return Ok(());
    }

    fn consume_escape(&mut self, byte: TaggedByte, emitted: &mut Vec<VtToken>) -> Result<()> {
        let value = byte.value;
        self.push(byte)?;
        match value {
            b'[' if self.token.len() == 2 => self.state = FramerState::Csi,
            b']' if self.token.len() == 2 => {
                self.state = FramerState::String {
                    kind: VtTokenKind::Osc,
                    osc: true,
                };
            }
            b'P' if self.token.len() == 2 => {
                self.state = FramerState::String {
                    kind: VtTokenKind::Dcs,
                    osc: false,
                };
            }
            b'_' if self.token.len() == 2 => {
                self.state = FramerState::String {
                    kind: VtTokenKind::Apc,
                    osc: false,
                };
            }
            0x30..=0x7e => {
                emitted.push(self.emit(VtTokenKind::Esc));
                self.state = FramerState::Ground;
            }
            0x20..=0x2f => {}
            _ => {
                emitted.push(self.emit(VtTokenKind::Unknown));
                self.state = FramerState::Ground;
            }
        }
        return Ok(());
    }

    fn consume_csi(&mut self, byte: TaggedByte, emitted: &mut Vec<VtToken>) -> Result<()> {
        let value = byte.value;
        self.push(byte)?;
        if (0x40..=0x7e).contains(&value) {
            emitted.push(self.emit(VtTokenKind::Csi));
            self.state = FramerState::Ground;
        } else if !(0x20..=0x3f).contains(&value) {
            emitted.push(self.emit(VtTokenKind::Unknown));
            self.state = FramerState::Ground;
        }
        return Ok(());
    }

    fn consume_string(
        &mut self,
        byte: TaggedByte,
        kind: VtTokenKind,
        osc: bool,
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        let value = byte.value;
        self.push(byte)?;
        if value == 0x9c || (osc && value == 0x07) {
            emitted.push(self.emit(kind));
            self.state = FramerState::Ground;
        } else if value == 0x1b {
            self.state = FramerState::StringEscape { kind, osc };
        }
        return Ok(());
    }

    fn begin_c1_sequence(
        &mut self,
        byte: TaggedByte,
        kind: VtTokenKind,
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        self.flush_printable(emitted);
        self.push(byte)?;
        self.state = match kind {
            VtTokenKind::Csi => FramerState::Csi,
            _ => FramerState::Ground,
        };
        return Ok(());
    }

    fn begin_string(
        &mut self,
        byte: TaggedByte,
        kind: VtTokenKind,
        osc: bool,
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        self.flush_printable(emitted);
        self.push(byte)?;
        self.state = FramerState::String { kind, osc };
        return Ok(());
    }

    fn push(&mut self, byte: TaggedByte) -> Result<()> {
        if self.token.len() >= self.max_token_bytes {
            return Err(Error::InvalidVtStream {
                reason: "VT token exceeds configured byte bound".to_owned(),
            });
        }
        self.token.push(byte.value);
        if let Some(last) = self.source.last_mut()
            && last.event_seq == byte.event_seq
            && last.end_byte_offset == byte.byte_offset
        {
            last.end_byte_offset = last.end_byte_offset.saturating_add(1);
        } else {
            self.source.push(VtSourceSegment {
                event_seq: byte.event_seq,
                start_byte_offset: byte.byte_offset,
                end_byte_offset: byte.byte_offset.saturating_add(1),
                monotonic_ns: byte.monotonic_ns,
            });
        }
        return Ok(());
    }

    fn flush_printable(&mut self, emitted: &mut Vec<VtToken>) {
        if !self.token.is_empty() {
            emitted.push(self.emit(VtTokenKind::Printable));
        }
        self.printable_utf8_remaining = 0;
    }

    fn emit(&mut self, kind: VtTokenKind) -> VtToken {
        let raw = mem::take(&mut self.token);
        let source = mem::take(&mut self.source);
        let description = describe(kind, &raw);
        let token = VtToken {
            token_id: self.next_token_id,
            kind,
            raw: ByteString::from(raw),
            source,
            description,
            timing_resolution: "event".to_owned(),
        };
        self.next_token_id = self.next_token_id.saturating_add(1);
        return token;
    }
}

/// Frame every canonical output event without changing or interpreting bytes.
///
/// # Errors
///
/// Returns an error when a token exceeds the one-MiB default bound.
pub fn frame_event_records(records: &[EventRecord]) -> Result<Vec<VtToken>> {
    let mut framer = VtFramer::default();
    let mut tokens = Vec::new();
    for record in records {
        if let Event::Output { data } = &record.event {
            tokens.extend(framer.feed(record.seq, record.monotonic_ns, data.as_bytes())?);
        }
    }
    tokens.extend(framer.finish()?);
    return Ok(tokens);
}

fn describe(kind: VtTokenKind, raw: &[u8]) -> String {
    if kind == VtTokenKind::Csi {
        let body = if raw.starts_with(b"\x1b[") {
            raw.get(2..).unwrap_or_default()
        } else {
            raw.get(1..).unwrap_or_default()
        };
        if let Some((&final_byte, parameters)) = body.split_last() {
            return match (parameters, final_byte) {
                (b"2", b'J') => "erase display (all)".to_owned(),
                (b"3", b'J') => "erase saved lines".to_owned(),
                (b"" | b"1;1", b'H') => "cursor home (1,1)".to_owned(),
                (b"?2026", b'h') => "set synchronized output".to_owned(),
                (b"?2026", b'l') => "reset synchronized output".to_owned(),
                _ => format!(
                    "CSI {} {}",
                    String::from_utf8_lossy(parameters),
                    char::from(final_byte)
                ),
            };
        }
    }
    return match kind {
        VtTokenKind::Printable => "printable bytes".to_owned(),
        VtTokenKind::C0 => format!("C0 0x{:02x}", raw.first().copied().unwrap_or(0)),
        VtTokenKind::C1 => format!("C1 0x{:02x}", raw.first().copied().unwrap_or(0)),
        VtTokenKind::Esc => "ESC sequence".to_owned(),
        VtTokenKind::Csi => "CSI sequence".to_owned(),
        VtTokenKind::Osc => "OSC string".to_owned(),
        VtTokenKind::Dcs => "DCS string".to_owned(),
        VtTokenKind::Apc => "APC string".to_owned(),
        VtTokenKind::Unknown => "unknown or malformed sequence".to_owned(),
        VtTokenKind::Incomplete => "incomplete sequence".to_owned(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_controls_losslessly_with_exact_segments() {
        let mut framer = VtFramer::default();
        let mut tokens = framer.feed(4, 10, b"A\x1b[2").expect("first read");
        tokens.extend(
            framer
                .feed(7, 20, b"J\x1b]0;title\x1b")
                .expect("second read"),
        );
        tokens.extend(framer.feed(8, 30, b"\\\xe7\x95\x8c").expect("third read"));
        tokens.extend(framer.finish().expect("finish"));
        assert_eq!(
            tokens
                .iter()
                .map(|token| return token.kind)
                .collect::<Vec<_>>(),
            [
                VtTokenKind::Printable,
                VtTokenKind::Csi,
                VtTokenKind::Osc,
                VtTokenKind::Printable,
            ]
        );
        assert_eq!(tokens[1].description, "erase display (all)");
        assert_eq!(tokens[1].source.len(), 2);
        assert_eq!(tokens[2].source.len(), 2);
        let recombined = tokens
            .iter()
            .flat_map(|token| return token.raw.as_bytes().iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(recombined, b"A\x1b[2J\x1b]0;title\x1b\\\xe7\x95\x8c");
    }

    #[test]
    fn preserves_noncontiguous_offsets_and_first_segment_timestamps() {
        let mut framer = VtFramer::default();
        for (event_seq, monotonic_ns, offset, byte) in [
            (4, 10, 5, b'A'),
            (4, 20, 6, b'B'),
            (4, 30, 12, b'C'),
            (7, 40, 0, b'D'),
            (4, 50, 13, b'E'),
        ] {
            assert!(
                framer
                    .feed_byte_at(event_seq, monotonic_ns, offset, byte)
                    .expect("feed positioned byte")
                    .is_empty()
            );
        }
        let tokens = framer.finish().expect("finish positioned bytes");
        let [token] = tokens.as_slice() else {
            panic!("printable stream must emit one token");
        };
        assert_eq!(token.raw.as_bytes(), b"ABCDE");
        assert_eq!(
            token.source,
            [
                VtSourceSegment {
                    event_seq: 4,
                    start_byte_offset: 5,
                    end_byte_offset: 7,
                    monotonic_ns: 10,
                },
                VtSourceSegment {
                    event_seq: 4,
                    start_byte_offset: 12,
                    end_byte_offset: 13,
                    monotonic_ns: 30,
                },
                VtSourceSegment {
                    event_seq: 7,
                    start_byte_offset: 0,
                    end_byte_offset: 1,
                    monotonic_ns: 40,
                },
                VtSourceSegment {
                    event_seq: 4,
                    start_byte_offset: 13,
                    end_byte_offset: 14,
                    monotonic_ns: 50,
                },
            ]
        );
    }

    #[test]
    fn labels_incomplete_and_bounds_large_strings() {
        let mut incomplete = VtFramer::default();
        assert!(
            incomplete
                .feed(1, 0, b"\x1b]unterminated")
                .expect("feed")
                .is_empty()
        );
        let token = incomplete.finish().expect("finish").remove(0);
        assert_eq!(token.kind, VtTokenKind::Incomplete);

        let mut bounded = VtFramer::new(4);
        assert!(bounded.feed(1, 0, b"\x1b]1234").is_err());
    }

    #[test]
    fn distinguishes_utf8_continuations_from_eight_bit_controls() {
        let mut framer = VtFramer::default();
        let mut tokens = framer
            .feed(1, 0, b"\xc2\x9b\x9b2J\x90data\x9c\x9fapp\x9c\xff\n")
            .expect("feed mixed stream");
        tokens.extend(framer.finish().expect("finish"));
        assert_eq!(
            tokens
                .iter()
                .map(|token| return token.kind)
                .collect::<Vec<_>>(),
            [
                VtTokenKind::Printable,
                VtTokenKind::Csi,
                VtTokenKind::Dcs,
                VtTokenKind::Apc,
                VtTokenKind::Printable,
                VtTokenKind::C0,
            ]
        );
        assert_eq!(tokens[1].description, "erase display (all)");
        let recombined = tokens
            .iter()
            .flat_map(|token| return token.raw.as_bytes().iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(recombined, b"\xc2\x9b\x9b2J\x90data\x9c\x9fapp\x9c\xff\n");
    }
}
