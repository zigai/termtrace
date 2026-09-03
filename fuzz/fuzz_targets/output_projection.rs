#![no_main]

use libfuzzer_sys::fuzz_target;
use termtrace_terminal::ControlStripper;

fuzz_target!(|input: &[u8]| {
    let (chunk_selector, bytes) = input
        .split_first()
        .map_or((0, &[][..]), |(first, rest)| (usize::from(*first), rest));
    let chunk_size = chunk_selector % 64 + 1;

    let mut contiguous = ControlStripper::default();
    let expected = contiguous.feed(bytes);

    let mut chunked = ControlStripper::default();
    let mut actual = Vec::new();
    for chunk in bytes.chunks(chunk_size) {
        actual.extend(chunked.feed(chunk));
    }

    assert_eq!(actual, expected);
    assert_eq!(
        chunked.alternate_screen_observed(),
        contiguous.alternate_screen_observed()
    );
    assert_eq!(chunked.source_byte_count(), bytes.len() as u64);
    assert_eq!(chunked.output_byte_count(), actual.len() as u64);
});
