//! Explicitly lossy, streaming-safe child-output projections.

pub use termtrace_model::protocol::output_projection::{
    OutputProjectionQuality, OutputProjectionReport,
};

/// Streaming control stripper retaining printable bytes, line breaks, carriage returns, and tabs.
#[derive(Debug, Default)]
pub struct ControlStripper {
    state: StripState,
    control: Vec<u8>,
    source_byte_count: u64,
    output_byte_count: u64,
    alternate_screen_observed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum StripState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    String,
    OscEscape,
    StringEscape,
}

impl ControlStripper {
    /// Consume one arbitrary byte chunk without requiring control boundaries to align.
    #[must_use]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.source_byte_count = self
            .source_byte_count
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let mut output = Vec::with_capacity(bytes.len());
        for byte in bytes.iter().copied() {
            self.consume(byte, &mut output);
        }
        self.output_byte_count = self
            .output_byte_count
            .saturating_add(u64::try_from(output.len()).unwrap_or(u64::MAX));
        return output;
    }

    /// Whether a recognized alternate-screen transition was observed.
    #[must_use]
    pub fn alternate_screen_observed(&self) -> bool {
        return self.alternate_screen_observed;
    }

    /// Total source bytes consumed.
    #[must_use]
    pub fn source_byte_count(&self) -> u64 {
        return self.source_byte_count;
    }

    /// Total bytes emitted.
    #[must_use]
    pub fn output_byte_count(&self) -> u64 {
        return self.output_byte_count;
    }

    fn consume(&mut self, byte: u8, output: &mut Vec<u8>) {
        match self.state {
            StripState::Ground => match byte {
                0x1b => self.begin(StripState::Escape, byte),
                0x9b => self.begin(StripState::Csi, byte),
                0x9d => self.begin(StripState::Osc, byte),
                0x90 | 0x98 | 0x9e | 0x9f => self.begin(StripState::String, byte),
                b'\n' | b'\r' | b'\t' => output.push(byte),
                0x00..=0x1f | 0x7f | 0x80..=0x9f => {}
                _ => output.push(byte),
            },
            StripState::Escape => {
                self.push_control(byte);
                self.state = match byte {
                    b'[' => StripState::Csi,
                    b']' => StripState::Osc,
                    b'P' | b'_' | b'^' | b'X' => StripState::String,
                    0x18 | 0x1a => {
                        self.finish_control();
                        StripState::Ground
                    }
                    0x30..=0x7e => {
                        self.finish_control();
                        StripState::Ground
                    }
                    _ => StripState::Escape,
                };
            }
            StripState::Csi => {
                self.push_control(byte);
                if byte == 0x18 || byte == 0x1a || (0x40..=0x7e).contains(&byte) {
                    self.finish_control();
                    self.state = StripState::Ground;
                }
            }
            StripState::Osc => {
                self.push_control(byte);
                match byte {
                    0x07 | 0x9c => {
                        self.finish_control();
                        self.state = StripState::Ground;
                    }
                    0x18 | 0x1a => {
                        self.finish_control();
                        self.state = StripState::Ground;
                    }
                    0x1b => self.state = StripState::OscEscape,
                    _ => {}
                }
            }
            StripState::String => {
                self.push_control(byte);
                match byte {
                    0x07 | 0x9c => {
                        self.finish_control();
                        self.state = StripState::Ground;
                    }
                    0x18 | 0x1a => {
                        self.finish_control();
                        self.state = StripState::Ground;
                    }
                    0x1b => self.state = StripState::StringEscape,
                    _ => {}
                }
            }
            StripState::OscEscape | StripState::StringEscape => {
                self.push_control(byte);
                if byte == b'\\' || byte == 0x18 || byte == 0x1a {
                    self.finish_control();
                    self.state = StripState::Ground;
                } else {
                    self.state = if self.state == StripState::OscEscape {
                        StripState::Osc
                    } else {
                        StripState::String
                    };
                }
            }
        }
    }

    fn begin(&mut self, state: StripState, byte: u8) {
        self.control.clear();
        self.control.push(byte);
        self.state = state;
    }

    fn push_control(&mut self, byte: u8) {
        if self.control.len() < 4096 {
            self.control.push(byte);
        }
    }

    fn finish_control(&mut self) {
        if self.control.ends_with(b"?47h")
            || self.control.ends_with(b"?1047h")
            || self.control.ends_with(b"?1049h")
        {
            self.alternate_screen_observed = true;
        }
        self.control.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    use proptest::{collection, prelude::*};

    #[test]
    fn strips_controls_across_chunks_and_flags_alternate_screen() {
        let mut stripper = ControlStripper::default();
        let mut output = stripper.feed(b"before\n\x1b[?104");
        output.extend(stripper.feed(b"9h\x1b[31mred\x1b[0m\n"));
        assert_eq!(output, b"before\nred\n");
        assert!(stripper.alternate_screen_observed());
        assert_eq!(stripper.source_byte_count(), 28);
        assert_eq!(stripper.output_byte_count(), 11);
    }

    #[test]
    fn strips_c0_and_eight_bit_control_families() {
        let mut stripper = ControlStripper::default();
        let output = stripper
            .feed(b"\x01\x7f\x80\x9b31mA\x9d0;title\x07B\x90payload\x1b\\C\x9fpayload\x1b\\D");

        assert_eq!(output, b"ABCD");
        assert!(!stripper.alternate_screen_observed());
    }

    #[test]
    fn strips_seven_bit_control_families_and_recovers_from_false_string_escapes() {
        let mut stripper = ControlStripper::default();
        let output = stripper
            .feed(b"\x1b]0;title\x07A\x1bPdata\x1b\\B\x1bcC\x1b]0;title\x1b\\D\x1b]0;a\x1bx\x07E");

        assert_eq!(output, b"ABCDE");
    }

    #[test]
    fn recognizes_each_supported_alternate_screen_sequence() {
        let untouched = ControlStripper::default();
        assert!(!untouched.alternate_screen_observed());

        for sequence in [b"\x1b[?47h".as_slice(), b"\x1b[?1047h", b"\x1b[?1049h"] {
            let mut stripper = ControlStripper::default();
            assert!(stripper.feed(sequence).is_empty());
            assert!(stripper.alternate_screen_observed());
        }
    }

    #[test]
    fn bounded_control_retention_does_not_classify_truncated_sequences() {
        let mut sequence = b"\x1b[".to_vec();
        sequence.extend(iter::repeat_n(b'0', 4_091));
        sequence.extend_from_slice(b"?47h");
        let mut stripper = ControlStripper::default();

        assert!(stripper.feed(&sequence).is_empty());
        assert!(!stripper.alternate_screen_observed());
    }

    #[test]
    fn recognizes_eight_bit_st_and_cancel_controls() {
        let mut stripper = ControlStripper::default();
        let output = stripper.feed(b"\x9d0;title\x9cHello\x1b[31\x18World\x1bXsos\x9cDone");
        assert_eq!(output, b"HelloWorldDone");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_chunking_preserves_projection(
            bytes in collection::vec(any::<u8>(), 0..512),
            chunk_size in 1_usize..64,
        ) {
            let mut contiguous = ControlStripper::default();
            let expected = contiguous.feed(&bytes);
            let expected_alternate_screen = contiguous.alternate_screen_observed();

            let mut chunked = ControlStripper::default();
            let mut actual = Vec::new();
            for chunk in bytes.chunks(chunk_size) {
                actual.extend(chunked.feed(chunk));
            }

            prop_assert_eq!(&actual, &expected);
            prop_assert_eq!(
                chunked.alternate_screen_observed(),
                expected_alternate_screen
            );
            prop_assert_eq!(chunked.source_byte_count(), bytes.len() as u64);
            prop_assert_eq!(chunked.output_byte_count(), actual.len() as u64);
        }
    }
}
