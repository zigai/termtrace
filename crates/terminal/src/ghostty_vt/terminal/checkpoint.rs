//! Exact engine checkpoints built on libghostty-vt's binary terminal snapshots.
//!
//! A checkpoint is a host header followed by one complete Ghostty snapshot, which also carries
//! the bytes of any unfinished sequence. The header is the host sequence state as one byte, a
//! flags byte, and the state's value as a little-endian `u32`: the bytes counted so far for an
//! OSC, or whether a dropped OSC tail just saw ESC. The flags record render-hold capture, effect
//! capture, and whether Ghostty already reported a processing error, which its snapshots drop.

use super::{
    check, sys, Callbacks, GhosttyVt, Sequence, CAN, ESC, MAX_CONTINUATION_BYTES,
    MAX_OSC_SEQUENCE_BYTES,
};
use crate::{Error, Result};
use std::{ffi::c_void, ptr, slice};

const HOST_HEADER_BYTES: usize = 6;
const RENDER_HOLDS: u8 = 1;
const EFFECTS: u8 = 2;
const PROCESSING_ERROR: u8 = 4;

impl GhosttyVt {
    /// Encode the complete engine state, including unfinished input, as a checkpoint.
    ///
    /// Returns `None` while an unfinished sequence is longer than the tracked continuation
    /// limit, because its earlier bytes can no longer be reproduced.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot encode the snapshot.
    pub fn encode_checkpoint(&self) -> Result<Option<Vec<u8>>> {
        if !continuation_available(self.terminal)? {
            return Ok(None);
        }

        let mut flags = 0;
        for (set, flag) in [
            (self.render_holds, RENDER_HOLDS),
            (self.captures_effects, EFFECTS),
            (
                self.processing_error || self.ghostty_processing_error(),
                PROCESSING_ERROR,
            ),
        ] {
            if set {
                flags |= flag;
            }
        }

        let mut bytes = encode_host_header(self.sequence, flags);
        let writer = sys::GhosttyWriter {
            write: Some(append_snapshot_bytes),
            userdata: (&raw mut bytes).cast::<c_void>(),
        };

        check("encode terminal snapshot", unsafe {
            sys::ghostty_snapshot_encode(self.terminal, writer)
        })?;

        return Ok(Some(bytes));
    }

    /// Restore an engine from [`GhosttyVt::encode_checkpoint`] output.
    ///
    /// Screens, history, cursor, modes, colors, title, unfinished input, render-hold and effect
    /// capture, and an already reported processing error come from the checkpoint. Host policy and callbacks are applied as by
    /// [`GhosttyVt::new`]. The terminfo name is host configuration that snapshots do not
    /// carry, so callers set it again.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed or truncated checkpoint bytes and libghostty-vt failures.
    pub fn restore_checkpoint(bytes: &[u8], max_scrollback: usize) -> Result<Self> {
        let (sequence, flags, snapshot) = decode_host_header(bytes)?;
        let terminal = decode_snapshot(snapshot)?;
        let callbacks = Callbacks {
            render_holds: flags & RENDER_HOLDS != 0,
            effects: flags & EFFECTS != 0,
        };
        let mut engine = Self::from_terminal(terminal, max_scrollback, callbacks)?;
        engine.sequence = sequence;
        engine.processing_error = flags & PROCESSING_ERROR != 0;

        match sequence {
            Sequence::Ground => {}
            Sequence::Open => engine.hold_probe.advance(&continuation(engine.terminal)?),
            Sequence::Osc { .. } | Sequence::DiscardOsc { .. } => engine.hold_probe.unknown(),
        }

        // The decoder replays unfinished input before callbacks exist, so Ghostty would never
        // report an unsupported control string in progress. CAN drops it unreported; replaying
        // the same bytes rebuilds the parser state with capture enabled. A partial UTF-8
        // character is left alone, since CAN would turn it into a replacement character.
        if callbacks.effects && sequence != Sequence::Ground {
            let continuation = continuation(engine.terminal)?;
            if continuation.first() == Some(&ESC) {
                engine.vt_write(&[CAN]);
                engine.vt_write(&continuation);
            }
        }

        return Ok(engine);
    }
}

fn encode_host_header(sequence: Sequence, flags: u8) -> Vec<u8> {
    let (state, value) = match sequence {
        Sequence::Ground => (0, 0),
        Sequence::Open => (1, 0),
        // The OSC cap keeps the count far below `u32::MAX`.
        Sequence::Osc { length } => (2, u32::try_from(length).unwrap_or(u32::MAX)),
        Sequence::DiscardOsc { escape } => (3, u32::from(escape)),
    };

    let mut bytes = Vec::with_capacity(HOST_HEADER_BYTES);
    bytes.extend_from_slice(&[state, flags]);
    bytes.extend_from_slice(&value.to_le_bytes());

    return bytes;
}

fn decode_host_header(bytes: &[u8]) -> Result<(Sequence, u8, &[u8])> {
    let Some((header, snapshot)) = bytes.split_first_chunk::<HOST_HEADER_BYTES>() else {
        return Err(invalid_checkpoint("truncated host header"));
    };
    let [state, flags, value @ ..] = *header;
    let value = usize::try_from(u32::from_le_bytes(value)).unwrap_or(usize::MAX);

    let sequence = match (state, value) {
        (0, 0) => Sequence::Ground,
        (1, 0) => Sequence::Open,
        (2, length) if (1..=MAX_OSC_SEQUENCE_BYTES).contains(&length) => Sequence::Osc { length },
        (3, escape @ (0 | 1)) => Sequence::DiscardOsc {
            escape: escape == 1,
        },
        _ => return Err(invalid_checkpoint("invalid host sequence state")),
    };

    if flags & !(RENDER_HOLDS | EFFECTS | PROCESSING_ERROR) != 0 {
        return Err(invalid_checkpoint("invalid host flags"));
    }
    return Ok((sequence, flags, snapshot));
}

fn continuation_available(terminal: sys::GhosttyTerminal) -> Result<bool> {
    let mut len = 0_usize;
    let result = unsafe {
        sys::ghostty_terminal_continuation_buf(terminal, ptr::null_mut(), 0, &raw mut len)
    };

    match result {
        sys::GhosttyResult_GHOSTTY_SUCCESS | sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE => {
            return Ok(true);
        }
        // Tracking is always enabled, so this means the retained prefix was dropped.
        sys::GhosttyResult_GHOSTTY_INVALID_VALUE => return Ok(false),
        result => {
            return Err(Error::GhosttyVt {
                operation: "size VT continuation",
                result,
            });
        }
    }
}

fn continuation(terminal: sys::GhosttyTerminal) -> Result<Vec<u8>> {
    let mut len = 0_usize;
    let result = unsafe {
        sys::ghostty_terminal_continuation_buf(terminal, ptr::null_mut(), 0, &raw mut len)
    };

    if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        check("size restored VT continuation", result)?;
    }

    if len == 0 {
        return Ok(Vec::new());
    }

    let mut bytes = vec![0; len];
    let mut written = 0_usize;
    check("copy restored VT continuation", unsafe {
        sys::ghostty_terminal_continuation_buf(
            terminal,
            bytes.as_mut_ptr(),
            bytes.len(),
            &raw mut written,
        )
    })?;
    bytes.truncate(written);

    return Ok(bytes);
}

fn decode_snapshot(bytes: &[u8]) -> Result<sys::GhosttyTerminal> {
    let decoder = SnapshotDecoder::new(bytes)?;
    decoder.set(
        "limit snapshot continuation",
        sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
        &MAX_CONTINUATION_BYTES,
    )?;
    decoder.set(
        "retain snapshot continuation",
        sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
        &true,
    )?;
    decoder.set(
        "disable restored Kitty graphics storage",
        sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT,
        &0_u64,
    )?;

    let mut terminal = ptr::null_mut();
    check("decode terminal snapshot", unsafe {
        sys::ghostty_snapshot_decoder_decode(decoder.0, &raw mut terminal)
    })?;

    let mut consumed = 0_usize;
    let consumed_result = check("get decoded snapshot length", unsafe {
        sys::ghostty_snapshot_decoder_get(
            decoder.0,
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET,
            (&raw mut consumed).cast::<c_void>(),
        )
    })
    .and_then(|()| {
        if consumed != bytes.len() {
            return Err(invalid_checkpoint("trailing bytes after terminal snapshot"));
        }
        return Ok(());
    });
    if let Err(error) = consumed_result {
        unsafe { sys::ghostty_terminal_free(terminal) };

        return Err(error);
    }
    return Ok(terminal);
}

struct SnapshotDecoder(sys::GhosttySnapshotDecoder);

impl SnapshotDecoder {
    fn new(bytes: &[u8]) -> Result<Self> {
        let mut decoder = ptr::null_mut();
        check("create snapshot decoder", unsafe {
            sys::ghostty_snapshot_decoder_new_buf(
                ptr::null(),
                &raw mut decoder,
                bytes.as_ptr(),
                bytes.len(),
            )
        })?;

        return Ok(Self(decoder));
    }

    fn set<T>(
        &self,
        operation: &'static str,
        option: sys::GhosttySnapshotDecoderOption,
        value: &T,
    ) -> Result<()> {
        return check(operation, unsafe {
            sys::ghostty_snapshot_decoder_set(self.0, option, ptr::from_ref(value).cast::<c_void>())
        });
    }
}

impl Drop for SnapshotDecoder {
    fn drop(&mut self) {
        unsafe { sys::ghostty_snapshot_decoder_free(self.0) };
    }
}

unsafe extern "C" fn append_snapshot_bytes(
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) -> bool {
    if userdata.is_null() || data.is_null() {
        return false;
    }

    let bytes = unsafe { &mut *userdata.cast::<Vec<u8>>() };
    bytes.extend_from_slice(unsafe { slice::from_raw_parts(data, len) });

    return true;
}

fn invalid_checkpoint(reason: &str) -> Error {
    return Error::TerminalOperation {
        operation: "decode engine checkpoint",
        reason: reason.to_owned(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty_vt::{Format, RenderHold};
    use termtrace_model::{
        execution::event::TerminalColors,
        terminal::state::{DirtyState, Rgb, SemanticState},
    };

    const SCROLLBACK: usize = 100_000;

    fn engine(cols: u16, rows: u16) -> GhosttyVt {
        return GhosttyVt::new(cols, rows, SCROLLBACK).expect("create terminal");
    }

    fn restore(engine: &GhosttyVt) -> GhosttyVt {
        let bytes = engine
            .encode_checkpoint()
            .expect("encode checkpoint")
            .expect("checkpoint available");

        return GhosttyVt::restore_checkpoint(&bytes, SCROLLBACK).expect("restore checkpoint");
    }

    fn content(engine: &mut GhosttyVt) -> (SemanticState, Vec<u8>) {
        let mut state = engine.snapshot().expect("snapshot");
        state.dirty = DirtyState::Clean;
        state.dirty_rows.clear();

        for row in &mut state.grid {
            row.dirty = false;
        }

        return (state, engine.format(Format::Vt).expect("format"));
    }

    // Writes `prefix` to a fresh engine, restores it from a checkpoint, writes `suffix` to both
    // the restored engine and an uninterrupted one, and returns both final engines.
    fn resume(mut original: GhosttyVt, prefix: &[u8], suffix: &[u8]) -> (GhosttyVt, GhosttyVt) {
        original.write(prefix);
        let mut restored = restore(&original);
        original.write(suffix);
        restored.write(suffix);

        assert_eq!(content(&mut restored), content(&mut original));

        return (original, restored);
    }

    fn plain(engine: &GhosttyVt) -> String {
        return String::from_utf8(engine.format(Format::Plain).expect("plain")).expect("utf-8");
    }

    #[test]
    fn leaving_a_checkpointed_alternate_screen_restores_the_primary_screen() {
        let (_, restored) = resume(
            engine(20, 4),
            b"primary text\r\n\x1b[?1049h\x1b[Halternate",
            b"\x1b[?1049l",
        );
        assert!(plain(&restored).contains("primary text"));
        assert!(!plain(&restored).contains("alternate"));
    }

    #[test]
    fn pending_wrap_survives_a_checkpoint() {
        let (_, restored) = resume(engine(4, 2), b"abcd", b"e");
        assert_eq!(plain(&restored), "abcd\ne");
    }

    #[test]
    fn unfinished_csi_and_utf8_input_continue_after_restore() {
        resume(engine(10, 2), b"x\x1b[3", b"1mred");

        let (_, restored) = resume(engine(10, 2), b"price \xe2\x82", b"\xac");
        assert_eq!(plain(&restored), "price \u{20ac}");
    }

    #[test]
    fn unfinished_osc_continues_after_restore() {
        let (_, mut restored) = resume(engine(10, 2), b"\x1b]2;ab", b"c\x07");
        assert_eq!(content(&mut restored).0.metadata.title.as_bytes(), b"abc");
        resume(engine(10, 2), b"x\x1b", b"]2;t\x07y");
    }

    #[test]
    fn oversized_osc_count_and_discard_state_survive_a_checkpoint() {
        let filler = vec![b'a'; MAX_OSC_SEQUENCE_BYTES - 8];
        let mut prefix = b"\x1b]2;".to_vec();
        prefix.extend_from_slice(&filler);

        // Restored mid-OSC, the cap must still trip after the remaining budget.
        let (_, mut restored) = resume(engine(10, 2), &prefix, b"0123456789\x9cok");
        assert!(content(&mut restored)
            .0
            .metadata
            .title
            .as_bytes()
            .is_empty());
        assert!(plain(&restored).starts_with("ok"));

        // Restored while dropping the tail, the tail must stay dropped.
        prefix.extend_from_slice(b"0123456789");
        let (_, mut restored) = resume(engine(10, 2), &prefix, b"tail\x9cok");
        assert!(content(&mut restored)
            .0
            .metadata
            .title
            .as_bytes()
            .is_empty());
        assert!(plain(&restored).starts_with("ok"));
    }

    #[test]
    fn restored_engine_keeps_capturing_render_holds() {
        let mut original = engine(10, 2);
        original.capture_render_holds().expect("capture holds");
        original.write(b"shown\x1b[?2026h");
        drop(original.take_render_holds().expect("hold start"));
        let mut restored = restore(&original);
        restored.write(b"\x1b[?2026l\x1b[?2026h");
        let holds = restored.take_render_holds().expect("holds after restore");
        assert!(matches!(
            holds.as_slice(),
            [RenderHold::Ended, RenderHold::Began(_)]
        ));
        assert!(!engine(10, 2).render_holds);
    }

    #[test]
    fn restored_engine_reports_the_same_later_effects_as_an_uninterrupted_one() {
        let cases: [(&[u8], &[u8]); 5] = [
            (b"\x1b]133;D;", b"7\x07\x07"),
            (b"\x1b]7400;a", b"b\x07"),
            (b"\x1b_Zcus", b"tom\x1b\\"),
            (b"\x1b]52;c;c2Vj", b"cmV0\x07"),
            (b"\x1b]2;one\x07", b"\x1b]9;4;3\x07\x1b[8;5t"),
        ];

        for (prefix, suffix) in cases {
            let mut original = engine(10, 2);
            original.capture_effects().expect("capture effects");
            original.write(prefix);
            drop(original.take_effects());
            let mut restored = restore(&original);

            original.write(suffix);
            restored.write(suffix);
            let effects = restored.take_effects();
            assert!(!effects.is_empty(), "no effects after {prefix:?}");
            assert_eq!(effects, original.take_effects());
        }

        // A partial UTF-8 character must come back as it was, not as a replacement character.
        let mut original = engine(10, 2);
        original.capture_effects().expect("capture effects");
        original.write(b"\xe7\x1b((0\xe7");
        let mut restored = restore(&original);
        original.write(b"\x9b\x81");
        restored.write(b"\x9b\x81");

        assert_eq!(content(&mut restored), content(&mut original));

        let mut plain = engine(10, 2);
        plain.write(b"\x07");
        let mut restored = restore(&plain);
        restored.write(b"\x07");

        assert!(restored.take_effects().is_empty());
    }

    #[test]
    fn resized_geometry_and_scrollback_survive_a_checkpoint() {
        let mut original = engine(10, 4);
        original
            .resize(6, 3, 8, 16)
            .expect("shrink before checkpoint");
        original.write(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");

        let mut restored = restore(&original);
        for terminal in [&mut original, &mut restored] {
            terminal.resize(6, 5, 8, 16).expect("grow after checkpoint");
            terminal.write(b"\x1b[14t");
        }

        assert_eq!(content(&mut restored), content(&mut original));
        assert_eq!(restored.take_responses(), original.take_responses());
        assert!(plain(&restored).starts_with("one"));
    }

    #[test]
    fn host_and_osc_colors_survive_a_checkpoint() {
        let colors = TerminalColors {
            source: "test".to_owned(),
            foreground: [1, 2, 3],
            background: [4, 5, 6],
            cursor: Some([7, 8, 9]),
            palette: (0..=255)
                .map(|index| return [index, index, index])
                .collect(),
        };

        let mut original = engine(10, 2);
        original
            .apply_terminal_colors(&colors)
            .expect("host colors");

        let (mut original, mut restored) = resume(
            original,
            b"\x1b]4;1;rgb:12/34/56\x07\x1b]10;rgb:aa/bb/cc\x07\x1b]11;rgb:01/02/03\x07",
            b"\x1b[31mX",
        );
        for terminal in [&mut original, &mut restored] {
            terminal.write(b"\x1b]104\x07\x1b]110\x07\x1b]111\x07");
        }

        let (state, _) = content(&mut restored);

        assert_eq!(content(&mut restored), content(&mut original));
        assert_eq!(state.colors.foreground, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(state.colors.background, Rgb { r: 4, g: 5, b: 6 });
    }

    #[test]
    fn restored_engine_reports_query_responses_with_host_terminfo() {
        let mut original = engine(10, 2);
        original.set_terminfo_name("xterm-test").expect("terminfo");
        original.write(b"ab");
        let mut restored = restore(&original);
        restored.set_terminfo_name("xterm-test").expect("terminfo");

        for terminal in [&mut original, &mut restored] {
            terminal.write(b"\x1b[6n\x1bP+q544e\x1b\\");
        }

        let responses = restored.take_responses();
        assert_eq!(responses, original.take_responses());
        assert!(responses.contains(&b"\x1b[1;3R".to_vec()));
        assert_eq!(responses.len(), 2);
    }

    #[test]
    fn rejects_truncated_and_padded_checkpoints() {
        let mut original = engine(10, 2);
        original.write(b"text");
        let bytes = original
            .encode_checkpoint()
            .expect("encode")
            .expect("available");

        let truncated = bytes.get(..bytes.len() - 1).expect("non-empty checkpoint");
        assert!(GhosttyVt::restore_checkpoint(truncated, SCROLLBACK).is_err());

        let mut padded = bytes.clone();
        padded.push(0);

        assert!(GhosttyVt::restore_checkpoint(&padded, SCROLLBACK).is_err());
        assert!(GhosttyVt::restore_checkpoint(&bytes[..3], SCROLLBACK).is_err());

        let mut bad_state = bytes.clone();
        bad_state[0] = 9;

        assert!(GhosttyVt::restore_checkpoint(&bad_state, SCROLLBACK).is_err());
    }
}
