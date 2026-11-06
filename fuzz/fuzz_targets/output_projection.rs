#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;
use termtrace_model::terminal::vt::VtToken;
use termtrace_terminal::{ControlStripper, GhosttyVt, VtFramer};

fn frame<'input>(chunks: impl Iterator<Item = &'input [u8]>) -> Vec<VtToken> {
    let mut framer = VtFramer::with_default_bound().expect("framer");
    let mut tokens = Vec::new();
    for chunk in chunks {
        tokens.extend(framer.feed(0, 0, chunk).expect("input below the token bound"));
    }
    tokens.extend(framer.finish().expect("finish"));

    return tokens;
}

fuzz_target!(|input: &[u8]| {
    let (chunk_selector, bytes) = input
        .split_first()
        .map_or((0, &[][..]), |(first, rest)| (usize::from(*first), rest));
    let chunk_size = chunk_selector % 64 + 1;

    let mut contiguous = ControlStripper::new().expect("stripper");
    let expected = contiguous.feed(bytes);

    let mut chunked = ControlStripper::new().expect("stripper");
    let mut actual = Vec::new();
    for chunk in bytes.chunks(chunk_size) {
        actual.extend(chunked.feed(chunk));
    }

    assert_eq!(actual, expected);
    assert_eq!(
        chunked.has_observed_alternate_screen(),
        contiguous.has_observed_alternate_screen()
    );
    assert_eq!(chunked.source_byte_count(), bytes.len() as u64);
    assert_eq!(chunked.output_byte_count(), actual.len() as u64);

    let tokens = frame(std::iter::once(bytes));
    let chunked_tokens = frame(bytes.chunks(chunk_size));
    let shape = |tokens: &[VtToken]| -> Vec<_> {
        return tokens
            .iter()
            .map(|token| (token.kind, token.raw.as_bytes().to_vec()))
            .collect();
    };
    assert_eq!(shape(&chunked_tokens), shape(&tokens));

    let mut token_ends = BTreeSet::new();
    let mut recombined = Vec::new();
    for token in &tokens {
        recombined.extend_from_slice(token.raw.as_bytes());
        token_ends.insert(recombined.len());
    }
    assert_eq!(recombined, bytes);

    // Ghostty's own stepping must agree on where text is and where sequences end.
    let mut stepped = GhosttyVt::new(80, 24, 1000).expect("valid fixed terminal geometry");
    let mut plain_text = Vec::new();
    let mut offset = 0;
    let mut rest = bytes;
    while !rest.is_empty() {
        let step = stepped.write_step(rest);
        plain_text.extend(rest[..step.plain].iter().copied().filter(|byte| {
            return matches!(byte, b'\n' | b'\r' | b'\t') || !matches!(byte, 0x00..=0x1f | 0x7f);
        }));
        offset += step.consumed;
        if !step.open && step.consumed > step.plain {
            assert!(token_ends.contains(&offset), "sequence end {offset} is not a token end");
        }
        rest = &rest[step.consumed..];
    }
    assert_eq!(expected, plain_text);
});
