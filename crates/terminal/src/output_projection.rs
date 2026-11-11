//! Explicitly lossy, streaming-safe child-output projections.

use crate::{
    ghostty_vt::{ScanKind, SequenceScanner},
    Result,
};
pub use termtrace_model::protocol::output_projection::{
    OutputProjectionQuality, OutputProjectionReport,
};

/// Streaming control stripper retaining printable bytes, line breaks, carriage returns, and tabs.
///
/// Ghostty's parser decides which bytes belong to control sequences. Bytes
/// outside them pass through unless they are C0 controls or DEL.
pub struct ControlStripper {
    scanner: SequenceScanner,
    source_byte_count: u64,
    output_byte_count: u64,
    alternate_screen_observed: bool,
}

impl ControlStripper {
    /// Create a stripper outside any control sequence.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate its parser.
    pub fn new() -> Result<Self> {
        return Ok(Self {
            scanner: SequenceScanner::new()?,
            source_byte_count: 0,
            output_byte_count: 0,
            alternate_screen_observed: false,
        });
    }

    /// Consume one arbitrary byte chunk without requiring control boundaries to align.
    #[must_use]
    pub fn feed(&mut self, mut bytes: &[u8]) -> Vec<u8> {
        self.source_byte_count = self
            .source_byte_count
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));

        let mut output = Vec::with_capacity(bytes.len());

        while !bytes.is_empty() {
            let step = self.scanner.scan_to_ground(bytes);
            let (consumed, rest) = bytes.split_at(step.consumed.min(bytes.len()));

            match step.kind {
                ScanKind::Text => output.extend(consumed.iter().copied().filter(|byte| {
                    return matches!(byte, b'\n' | b'\r' | b'\t')
                        || !matches!(byte, 0x00..=0x1f | 0x7f);
                })),
                ScanKind::Sequence { ground, .. } => {
                    if ground && !self.alternate_screen_observed {
                        self.alternate_screen_observed = self.scanner.alternate_screen();
                    }
                }
            }

            bytes = rest;
        }

        self.output_byte_count = self
            .output_byte_count
            .saturating_add(u64::try_from(output.len()).unwrap_or(u64::MAX));
        return output;
    }

    /// Whether the program switched to the alternate screen at some point.
    #[must_use]
    pub fn has_observed_alternate_screen(&self) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection, prelude::*};

    #[test]
    fn strips_controls_across_chunks_and_flags_alternate_screen() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let mut output = stripper.feed(b"before\n\x1b[?104");
        output.extend(stripper.feed(b"9h\x1b[31mred\x1b[0m\n"));
        assert_eq!(output, b"before\nred\n");
        assert!(stripper.has_observed_alternate_screen());
        assert_eq!(stripper.source_byte_count(), 28);
        assert_eq!(stripper.output_byte_count(), 11);
    }

    #[test]
    fn keeps_eight_bit_bytes_outside_sequences_as_text() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper.feed(b"\x01\x7f\x80\x9b31mA\x9d0;title\x07B");
        assert_eq!(output, b"\x80\x9b31mA\x9d0;titleB");
        assert!(!stripper.has_observed_alternate_screen());
    }

    #[test]
    fn strips_eight_bit_introducers_inside_sequences() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper.feed(b"\x1b[1\x9d0;title\x07A\x1b_app\x90payload\x1b\\B");
        assert_eq!(output, b"AB");
    }

    #[test]
    fn dcs_payload_ends_only_at_esc_can_or_sub() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper.feed(b"\x1bPq\xc4\x9cTAIL\x9c\x07\x1b\\after");
        assert_eq!(output, b"after");
    }

    #[test]
    fn strips_seven_bit_control_families_and_recovers_from_false_string_escapes() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper
            .feed(b"\x1b]0;title\x07A\x1bPdata\x1b\\B\x1bcC\x1b]0;title\x1b\\D\x1b]0;a\x1bx\x07E");
        assert_eq!(output, b"ABCDE");
    }

    #[test]
    fn cancellation_preserves_text_after_incomplete_controls_across_chunk_boundaries() {
        for (name, prefix) in [
            ("escape", b"\x1b".as_slice()),
            ("escape intermediate", b"\x1b("),
            ("csi", b"\x1b[31"),
            ("osc", b"\x1b]0;title"),
            ("dcs", b"\x1bPpayload"),
            ("osc pending escape", b"\x1b]0;title\x1b"),
            ("dcs pending escape", b"\x1bPpayload\x1b"),
        ] {
            for cancel in [0x18, 0x1a] {
                let mut input = b"before".to_vec();
                input.extend_from_slice(prefix);
                input.push(cancel);
                input.extend_from_slice(b"after\n");

                for split in 0..=input.len() {
                    let mut stripper = ControlStripper::new().expect("stripper");
                    let mut output = stripper.feed(&input[..split]);
                    output.extend(stripper.feed(&input[split..]));
                    assert_eq!(
                        output, b"beforeafter\n",
                        "{name}, cancellation {cancel:#x}, split {split}"
                    );
                }
            }
        }
    }

    #[test]
    fn utf8_continuation_in_osc_does_not_leak_title_text_across_chunk_boundaries() {
        let input = b"before\x1b]0;\xc4\x9cprivate title\x07after\n";

        for split in 0..=input.len() {
            let mut stripper = ControlStripper::new().expect("stripper");
            let mut output = stripper.feed(&input[..split]);
            output.extend(stripper.feed(&input[split..]));
            assert_eq!(output, b"beforeafter\n", "split {split}");
        }
    }

    #[test]
    fn esc_inside_osc_ends_the_string_and_starts_an_escape_sequence() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper.feed(b"before\x1b]0;prefix\x1bxshown\x07after\n");
        assert_eq!(output, b"beforeshownafter\n");
    }

    #[test]
    fn recognizes_each_supported_alternate_screen_sequence() {
        let untouched = ControlStripper::new().expect("stripper");
        assert!(!untouched.has_observed_alternate_screen());

        for sequence in [
            b"\x1b[?47h".as_slice(),
            b"\x1b[?1047h",
            b"\x1b[?1049h",
            b"\x1b[?25;1049h",
        ] {
            let mut stripper = ControlStripper::new().expect("stripper");
            assert!(stripper.feed(sequence).is_empty());
            assert!(stripper.has_observed_alternate_screen());
        }
    }

    #[test]
    fn alternate_screen_text_outside_a_mode_set_does_not_count() {
        for sequence in [b"\x1b]0;?1049h\x07".as_slice(), b"\x1b[?1049l", b"?1049h"] {
            let mut stripper = ControlStripper::new().expect("stripper");
            let output = stripper.feed(sequence);
            assert!(!stripper.has_observed_alternate_screen(), "{output:?}");
        }
    }

    #[test]
    fn recognizes_eight_bit_st_and_cancel_controls() {
        let mut stripper = ControlStripper::new().expect("stripper");
        let output = stripper.feed(b"\x1b_app\x9cHello\x1b[31\x18World\x1bXsos\x9cDone");
        assert_eq!(output, b"HelloWorldDone");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_chunking_preserves_projection(
            bytes in collection::vec(any::<u8>(), 0..512),
            chunk_size in 1_usize..64,
        ) {
            let mut contiguous = ControlStripper::new().expect("stripper");
            let expected = contiguous.feed(&bytes);
            let expected_alternate_screen = contiguous.has_observed_alternate_screen();

            let mut chunked = ControlStripper::new().expect("stripper");
            let mut actual = Vec::new();
            for chunk in bytes.chunks(chunk_size) {
                actual.extend(chunked.feed(chunk));
            }

            prop_assert_eq!(&actual, &expected);
            prop_assert_eq!(
                chunked.has_observed_alternate_screen(),
                expected_alternate_screen
            );
            prop_assert_eq!(chunked.source_byte_count(), bytes.len() as u64);
            prop_assert_eq!(chunked.output_byte_count(), actual.len() as u64);
        }
    }
}
