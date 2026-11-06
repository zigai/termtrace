//! Lossless streaming framing of VT output bytes at Ghostty's sequence boundaries.

use crate::{
    ghostty_vt::{ControlString, ScanKind, SequenceScanner, StringEnd, StringStart},
    Error, Result,
};
use std::mem;
use termtrace_model::execution::event::{Event, EventRecord};
pub use termtrace_model::terminal::vt::{VtSourceSegment, VtToken, VtTokenKind};
use termtrace_model::value::ByteString;

const DEFAULT_MAX_TOKEN_BYTES: usize = 1_048_576;
const ESC: u8 = 0x1b;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Open {
    Empty,
    Printable,
    Sequence,
    String {
        kind: ControlString,
        // The string ended at an ESC, the token's last byte; a following `\` completes ST.
        escape: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct Position {
    event_seq: u64,
    monotonic_ns: u64,
    byte_offset: u64,
}

impl Position {
    fn advance(&mut self, bytes: usize) {
        self.byte_offset = self
            .byte_offset
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }
}

/// Bounded stateful VT framer that accepts successive canonical output reads.
///
/// Ghostty's parser decides where control sequences start and end. Text
/// outside them is split only at C0 controls.
pub struct VtFramer {
    scanner: SequenceScanner,
    open: Open,
    token: Vec<u8>,
    source: Vec<VtSourceSegment>,
    next_token_id: u64,
    max_token_bytes: usize,
}

impl VtFramer {
    /// Construct a framer with an explicit maximum buffered token size.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate its parser.
    pub fn new(max_token_bytes: usize) -> Result<Self> {
        return Ok(Self {
            scanner: SequenceScanner::new()?,
            open: Open::Empty,
            token: Vec::new(),
            source: Vec::new(),
            next_token_id: 0,
            max_token_bytes,
        });
    }

    /// Construct a framer that buffers at most one MiB per token.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate its parser.
    pub fn with_default_bound() -> Result<Self> {
        return Self::new(DEFAULT_MAX_TOKEN_BYTES);
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
        let position = Position {
            event_seq,
            monotonic_ns,
            byte_offset: 0,
        };
        self.consume(position, bytes, &mut emitted)?;

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
        let position = Position {
            event_seq,
            monotonic_ns,
            byte_offset,
        };
        self.consume(position, &[value], &mut emitted)?;

        return Ok(emitted);
    }

    /// Finish the stream, emitting printable bytes and labeling open controls incomplete.
    ///
    /// # Errors
    ///
    /// Shares the fallible signature of the other framing operations; finishing itself
    /// buffers nothing new.
    pub fn finish(mut self) -> Result<Vec<VtToken>> {
        let mut emitted = Vec::new();

        match self.open {
            Open::Empty => {}
            Open::Printable => emitted.push(self.emit(VtTokenKind::Printable, None)),
            Open::Sequence | Open::String { .. } => {
                emitted.push(self.emit(VtTokenKind::Incomplete, None));
            }
        }

        return Ok(emitted);
    }

    fn consume(
        &mut self,
        mut position: Position,
        mut bytes: &[u8],
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            let step = self.scanner.scan(bytes);
            let (consumed, rest) = bytes.split_at(step.consumed.min(bytes.len()));

            match step.kind {
                ScanKind::Text => self.text(position, consumed, emitted)?,
                ScanKind::Sequence {
                    escape,
                    ground,
                    start,
                    end,
                } => self.sequence(position, consumed, escape, ground, start, end, emitted)?,
            }

            position.advance(consumed.len());
            bytes = rest;
        }

        return Ok(());
    }

    fn text(
        &mut self,
        mut position: Position,
        mut bytes: &[u8],
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            let control = bytes
                .iter()
                .position(|byte| return matches!(byte, 0x00..=0x1f | 0x7f));
            let length = if control == Some(0) {
                self.flush(emitted);
                self.push(position, bytes.get(..1).unwrap_or_default())?;
                emitted.push(self.emit(VtTokenKind::C0, None));

                1
            } else {
                let length = control.unwrap_or(bytes.len());
                self.push(position, bytes.get(..length).unwrap_or_default())?;
                self.open = Open::Printable;

                length
            };
            position.advance(length);
            bytes = bytes.get(length..).unwrap_or_default();
        }

        return Ok(());
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments are the fields of one parser step"
    )]
    fn sequence(
        &mut self,
        mut position: Position,
        mut bytes: &[u8],
        escape: bool,
        ground: bool,
        start: Option<StringStart>,
        end: Option<StringEnd>,
        emitted: &mut Vec<VtToken>,
    ) -> Result<()> {
        if let Open::String { kind, escape: true } = self.open {
            if !escape && bytes.first() == Some(&b'\\') {
                self.push(position, bytes)?;
                emitted.push(self.emit(string_token_kind(kind), Some(kind)));

                return Ok(());
            }

            // The ESC that ended the string starts another sequence rather than ST.
            let terminator = self.split_last();
            emitted.push(self.emit(string_token_kind(kind), Some(kind)));

            if let Some((byte, segment)) = terminator {
                self.token.push(byte);
                self.source.push(segment);
            }

            self.open = Open::Sequence;
        }

        if escape {
            self.flush(emitted);
            self.open = Open::Sequence;
        }

        // An 8-bit control that starts another sequence cuts the open one short.
        let cut = start.is_some_and(|start| return start.eight_bit)
            || (end == Some(StringEnd::Aborted) && !ground);

        if cut && let Some(split) = bytes.len().checked_sub(1) {
            let (leading, last) = bytes.split_at(split);
            self.push(position, leading)?;
            self.flush(emitted);
            position.advance(leading.len());
            bytes = last;
            self.open = Open::Sequence;
        }

        self.push(position, bytes)?;

        if let Some(start) = start {
            self.open = Open::String {
                kind: start.kind,
                escape: false,
            };
        }

        if end == Some(StringEnd::Escape)
            && let Open::String { kind, .. } = self.open
        {
            self.open = Open::String { kind, escape: true };
        }

        if ground {
            let token = match self.open {
                Open::String { kind, .. }
                    if matches!(end, Some(StringEnd::Bell | StringEnd::Terminator)) =>
                {
                    self.emit(string_token_kind(kind), Some(kind))
                }
                Open::Sequence => {
                    let kind = sequence_kind(&self.token);

                    self.emit(kind, None)
                }
                Open::Empty | Open::Printable | Open::String { .. } => {
                    self.emit(VtTokenKind::Unknown, None)
                }
            };
            emitted.push(token);
        }

        return Ok(());
    }

    fn flush(&mut self, emitted: &mut Vec<VtToken>) {
        let kind = match self.open {
            Open::Empty => return,
            Open::Printable => VtTokenKind::Printable,
            Open::Sequence | Open::String { .. } => VtTokenKind::Unknown,
        };
        emitted.push(self.emit(kind, None));
    }

    fn push(&mut self, position: Position, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        if self.token.len().saturating_add(bytes.len()) > self.max_token_bytes {
            return Err(Error::InvalidVtStream {
                reason: "VT token exceeds configured byte bound".to_owned(),
            });
        }

        self.token.extend_from_slice(bytes);

        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);

        if let Some(last) = self.source.last_mut()
            && last.event_seq == position.event_seq
            && last.end_byte_offset == position.byte_offset
        {
            last.end_byte_offset = last.end_byte_offset.saturating_add(length);
        } else {
            self.source.push(VtSourceSegment {
                event_seq: position.event_seq,
                start_byte_offset: position.byte_offset,
                end_byte_offset: position.byte_offset.saturating_add(length),
                monotonic_ns: position.monotonic_ns,
            });
        }

        return Ok(());
    }

    fn split_last(&mut self) -> Option<(u8, VtSourceSegment)> {
        let byte = self.token.pop()?;
        let last = self.source.last_mut()?;
        let segment = VtSourceSegment {
            event_seq: last.event_seq,
            start_byte_offset: last.end_byte_offset.saturating_sub(1),
            end_byte_offset: last.end_byte_offset,
            monotonic_ns: last.monotonic_ns,
        };
        last.end_byte_offset = segment.start_byte_offset;

        if last.start_byte_offset == last.end_byte_offset {
            self.source.pop();
        }

        return Some((byte, segment));
    }

    fn emit(&mut self, kind: VtTokenKind, string: Option<ControlString>) -> VtToken {
        let raw = mem::take(&mut self.token);
        let source = mem::take(&mut self.source);
        let description = match (kind, string) {
            (VtTokenKind::Apc, Some(ControlString::Sos)) => "SOS string".to_owned(),
            (VtTokenKind::Apc, Some(ControlString::Pm)) => "PM string".to_owned(),
            _ => describe(kind, &raw),
        };

        let token = VtToken {
            token_id: self.next_token_id,
            kind,
            raw: ByteString::from(raw),
            source,
            description,
            timing_resolution: "event".to_owned(),
        };

        self.open = Open::Empty;
        self.next_token_id = self.next_token_id.saturating_add(1);

        return token;
    }
}

/// Frame every canonical output event without changing or interpreting bytes.
///
/// # Errors
///
/// Returns an error when the parser cannot be allocated or a token exceeds the one-MiB
/// default bound.
pub fn frame_event_records(records: &[EventRecord]) -> Result<Vec<VtToken>> {
    let mut framer = VtFramer::with_default_bound()?;
    let mut tokens = Vec::new();

    for record in records {
        if let Event::Output { data } = &record.event {
            tokens.extend(framer.feed(record.seq, record.monotonic_ns, data.as_bytes())?);
        }
    }

    tokens.extend(framer.finish()?);

    return Ok(tokens);
}

fn string_token_kind(kind: ControlString) -> VtTokenKind {
    return match kind {
        ControlString::Osc => VtTokenKind::Osc,
        ControlString::Dcs => VtTokenKind::Dcs,
        ControlString::Apc | ControlString::Sos | ControlString::Pm => VtTokenKind::Apc,
    };
}

fn sequence_kind(raw: &[u8]) -> VtTokenKind {
    let Some(&last) = raw.last() else {
        return VtTokenKind::Unknown;
    };

    if raw.starts_with(b"\x1b[") || raw.first() == Some(&0x9b) {
        return if raw.len() > 1 && (0x40..=0x7e).contains(&last) {
            VtTokenKind::Csi
        } else {
            VtTokenKind::Unknown
        };
    }

    if raw.first() == Some(&ESC) && raw.len() > 1 && (0x30..=0x7e).contains(&last) {
        return VtTokenKind::Esc;
    }
    return VtTokenKind::Unknown;
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
    use proptest::{collection, prelude::*};

    #[test]
    fn frames_split_controls_losslessly_with_exact_segments() {
        let mut framer = VtFramer::with_default_bound().expect("framer");
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
        let mut framer = VtFramer::with_default_bound().expect("framer");
        for (event_seq, monotonic_ns, offset, byte) in [
            (4, 10, 5, b'A'),
            (4, 20, 6, b'B'),
            (4, 30, 12, b'C'),
            (7, 40, 0, b'D'),
            (4, 50, 13, b'E'),
        ] {
            assert!(framer
                .feed_byte_at(event_seq, monotonic_ns, offset, byte)
                .expect("feed positioned byte")
                .is_empty());
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
    fn osc_utf8_continuation_does_not_terminate_the_token() {
        let mut framer = VtFramer::with_default_bound().expect("framer");
        let mut tokens = framer.feed(1, 0, b"\x1b]0;\xc4").expect("first read");
        tokens.extend(framer.feed(2, 1, b"\x9crest\x07").expect("second read"));
        tokens.extend(framer.finish().expect("finish"));

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, VtTokenKind::Osc);
        assert_eq!(tokens[0].raw.as_bytes(), b"\x1b]0;\xc4\x9crest\x07");
    }

    #[test]
    fn labels_incomplete_and_bounds_large_strings() {
        let mut incomplete = VtFramer::with_default_bound().expect("framer");
        assert!(incomplete
            .feed(1, 0, b"\x1b]unterminated")
            .expect("feed")
            .is_empty());

        let token = incomplete.finish().expect("finish").remove(0);
        assert_eq!(token.kind, VtTokenKind::Incomplete);

        let mut bounded = VtFramer::new(4).expect("framer");
        assert!(bounded.feed(1, 0, b"\x1b]1234").is_err());
    }

    fn kinds_and_bytes(tokens: &[VtToken]) -> Vec<(VtTokenKind, &[u8])> {
        return tokens
            .iter()
            .map(|token| return (token.kind, token.raw.as_bytes()))
            .collect();
    }

    fn frame(bytes: &[u8]) -> Vec<VtToken> {
        let mut framer = VtFramer::with_default_bound().expect("framer");
        let mut tokens = framer.feed(1, 0, bytes).expect("feed");
        tokens.extend(framer.finish().expect("finish"));

        return tokens;
    }

    #[test]
    fn eight_bit_bytes_outside_sequences_are_text() {
        let tokens = frame(b"\xc2\x9b\x9b2J\x90data\x9c\x9fapp\x9c\xff\n");
        assert_eq!(
            kinds_and_bytes(&tokens),
            [
                (
                    VtTokenKind::Printable,
                    b"\xc2\x9b\x9b2J\x90data\x9c\x9fapp\x9c\xff".as_slice()
                ),
                (VtTokenKind::C0, b"\n"),
            ]
        );
    }

    #[test]
    fn eight_bit_introducer_inside_a_sequence_starts_a_new_token() {
        let tokens = frame(b"\x1b[1\x9d0;t\x07\x1b_a\x9b2J");
        assert_eq!(
            kinds_and_bytes(&tokens),
            [
                (VtTokenKind::Unknown, b"\x1b[1".as_slice()),
                (VtTokenKind::Osc, b"\x9d0;t\x07"),
                (VtTokenKind::Unknown, b"\x1b_a"),
                (VtTokenKind::Csi, b"\x9b2J"),
            ]
        );
        assert_eq!(tokens[3].description, "erase display (all)");
    }

    #[test]
    fn esc_interrupts_sequences_and_ends_strings() {
        let tokens = frame(b"\x1b[3\x1b[2J\x1b]0;t\x1b[H\x1bPq\x1b\\\x1bXs\x1b\\\x1b^p\x07\x9c");
        assert_eq!(
            kinds_and_bytes(&tokens),
            [
                (VtTokenKind::Unknown, b"\x1b[3".as_slice()),
                (VtTokenKind::Csi, b"\x1b[2J"),
                (VtTokenKind::Osc, b"\x1b]0;t"),
                (VtTokenKind::Csi, b"\x1b[H"),
                (VtTokenKind::Dcs, b"\x1bPq\x1b\\"),
                (VtTokenKind::Apc, b"\x1bXs\x1b\\"),
                (VtTokenKind::Apc, b"\x1b^p\x07\x9c"),
            ]
        );
        assert_eq!(tokens[5].description, "SOS string");
        assert_eq!(tokens[6].description, "PM string");
    }

    #[test]
    fn cancelled_controls_are_unknown_and_text_resumes() {
        let tokens = frame(b"\x1b]0;t\x18A\x1b[31\x1aB");
        assert_eq!(
            kinds_and_bytes(&tokens),
            [
                (VtTokenKind::Unknown, b"\x1b]0;t\x18".as_slice()),
                (VtTokenKind::Printable, b"A"),
                (VtTokenKind::Unknown, b"\x1b[31\x1a"),
                (VtTokenKind::Printable, b"B"),
            ]
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_chunking_frames_identical_lossless_tokens(
            bytes in collection::vec(any::<u8>(), 0..512),
            chunk_size in 1_usize..64,
        ) {
            let expected = frame(&bytes);

            let mut framer = VtFramer::with_default_bound().expect("framer");
            let mut actual = Vec::new();
            for chunk in bytes.chunks(chunk_size) {
                actual.extend(framer.feed(1, 0, chunk).expect("feed"));
            }
            actual.extend(framer.finish().expect("finish"));

            let recombined = expected
                .iter()
                .flat_map(|token| return token.raw.as_bytes().iter().copied())
                .collect::<Vec<_>>();
            prop_assert_eq!(&recombined, &bytes);
            prop_assert_eq!(kinds_and_bytes(&actual), kinds_and_bytes(&expected));
        }
    }
}
