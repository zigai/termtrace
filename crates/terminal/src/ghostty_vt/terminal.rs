//! Terminal-handle lifecycle, bounded VT ingestion, and host resource policy.

use super::{
    input, snapshot, sys,
    types::{
        supported_geometry_cell_count, EncodedKey, Format, KeyModifiers, MouseAction, MouseButton,
        NamedKey, SemanticState,
    },
};
use crate::{Error, Result};
use std::{ffi::c_void, mem::take, ptr, slice};
use termtrace_model::execution::event::TerminalColors;
use termtrace_model::value::ByteString;

// Keep allocating control strings below the same ceiling as explanation framing.
const MAX_OSC_SEQUENCE_BYTES: usize = 1024 * 1024;
const MAX_APC_SEQUENCE_BYTES: usize = 1024 * 1024;
const TERMINAL_WRITE_CHUNK_BYTES: usize = 4 * 1024;

/// Owned semantic engine and render snapshot state.
pub struct GhosttyVt {
    terminal: sys::GhosttyTerminal,
    render: sys::GhosttyRenderState,
    last_snapshot: Option<SemanticState>,
    responses: Box<ResponseState>,
    input_limiter: VtInputLimiter,
}

#[derive(Default)]
struct ResponseState {
    pending: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum VtInputState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

#[derive(Debug)]
struct VtInputLimiter {
    state: VtInputState,
    sequence: Vec<u8>,
    discarding: bool,
    max_osc_bytes: usize,
    utf8_continuations: u8,
}

impl Default for VtInputLimiter {
    fn default() -> Self {
        return Self::new(MAX_OSC_SEQUENCE_BYTES);
    }
}

impl VtInputLimiter {
    fn new(max_osc_bytes: usize) -> Self {
        return Self {
            state: VtInputState::Ground,
            sequence: Vec::new(),
            discarding: false,
            max_osc_bytes,
            utf8_continuations: 0,
        };
    }

    fn write(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        let mut output = [0_u8; TERMINAL_WRITE_CHUNK_BYTES];
        let mut output_len = 0;

        for &byte in bytes {
            if self.state == VtInputState::Ground && self.utf8_continuations > 0 {
                if (0x80..=0xbf).contains(&byte) {
                    self.utf8_continuations -= 1;
                    push_output(byte, &mut output, &mut output_len, &mut emit);
                    continue;
                }

                self.utf8_continuations = 0;
            }

            match self.state {
                VtInputState::Ground => match byte {
                    0x1b => {
                        flush_output(&output, &mut output_len, &mut emit);
                        self.sequence.clear();
                        self.discarding = false;
                        let _ = self.push_sequence(byte);
                        self.state = VtInputState::Escape;
                    }
                    0x9d => {
                        flush_output(&output, &mut output_len, &mut emit);
                        self.begin_osc(byte);
                    }
                    value => {
                        self.utf8_continuations = match value {
                            0xc2..=0xdf => 1,
                            0xe0..=0xef => 2,
                            0xf0..=0xf4 => 3,
                            _ => 0,
                        };

                        push_output(byte, &mut output, &mut output_len, &mut emit);
                    }
                },
                VtInputState::Escape => match byte {
                    b']' => {
                        let _ = self.push_sequence(byte);
                        self.state = VtInputState::Osc;
                    }
                    0x1b => {
                        self.sequence.clear();
                        self.discarding = false;
                        push_output(0x1b, &mut output, &mut output_len, &mut emit);

                        let _ = self.push_sequence(byte);
                    }
                    _ => {
                        self.sequence.clear();
                        self.discarding = false;
                        push_output(0x1b, &mut output, &mut output_len, &mut emit);
                        push_output(byte, &mut output, &mut output_len, &mut emit);
                        self.state = VtInputState::Ground;
                    }
                },
                VtInputState::Osc | VtInputState::OscEscape => {
                    self.consume_osc(byte, &mut emit);
                }
            }
        }

        flush_output(&output, &mut output_len, &mut emit);
    }

    fn begin_osc(&mut self, introducer: u8) {
        self.sequence.clear();
        self.discarding = false;
        let _ = self.push_sequence(introducer);
        self.state = VtInputState::Osc;
    }

    fn consume_osc(&mut self, byte: u8, emit: &mut impl FnMut(&[u8])) {
        let terminated =
            matches!(byte, 0x07 | 0x9c) || (self.state == VtInputState::OscEscape && byte == b'\\');
        let canceled = matches!(byte, 0x18 | 0x1a);
        let was_discarding = self.discarding;
        let overflowed = self.push_sequence(byte);

        if overflowed {
            // Ghostty may be inside an earlier partial control. CAN returns its
            // parser to ground without executing the discarded OSC payload.
            emit(&[0x18]);
        }

        if terminated {
            if !self.discarding {
                emit(&self.sequence);
            }

            self.reset();
        } else if canceled {
            if !was_discarding && !overflowed {
                emit(&[byte]);
            }

            self.reset();
        } else {
            self.state = if byte == 0x1b { VtInputState::OscEscape } else { VtInputState::Osc };
        }
    }

    fn push_sequence(&mut self, byte: u8) -> bool {
        if self.discarding {
            return false;
        }

        if self.sequence.len() >= self.max_osc_bytes {
            self.sequence.clear();
            self.discarding = true;
            return true;
        }

        self.sequence.push(byte);
        return false;
    }

    fn reset(&mut self) {
        self.state = VtInputState::Ground;
        self.sequence.clear();
        self.discarding = false;
        self.utf8_continuations = 0;
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "output_len is bounded by TERMINAL_WRITE_CHUNK_BYTES"
)]
fn push_output(
    byte: u8,
    output: &mut [u8; TERMINAL_WRITE_CHUNK_BYTES],
    output_len: &mut usize,
    emit: &mut impl FnMut(&[u8]),
) {
    if *output_len == output.len() {
        flush_output(output, output_len, emit);
    }

    output[*output_len] = byte;
    *output_len += 1;
}

fn flush_output(
    output: &[u8; TERMINAL_WRITE_CHUNK_BYTES],
    output_len: &mut usize,
    emit: &mut impl FnMut(&[u8]),
) {
    if *output_len == 0 {
        return;
    }

    if let Some(bytes) = output.get(..*output_len) {
        emit(bytes);
    }

    *output_len = 0;
}

impl GhosttyVt {
    /// Create an engine with the given terminal geometry and scrollback limit.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate either handle.
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self> {
        if supported_geometry_cell_count(cols, rows).is_none() {
            return Err(Error::GhosttyVt {
                operation: "validate geometry",
                result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }

        let mut terminal = ptr::null_mut();
        check("create terminal", unsafe {
            sys::ghostty_terminal_new(
                ptr::null(),
                &raw mut terminal,
                sys::GhosttyTerminalOptions {
                    cols,
                    rows,
                    max_scrollback,
                },
            )
        })?;

        if let Err(error) = configure_terminal_resource_policy(terminal) {
            unsafe { sys::ghostty_terminal_free(terminal) };
            return Err(error);
        }

        let mut responses = Box::<ResponseState>::default();
        let userdata = (&raw mut *responses).cast::<c_void>();

        if let Err(error) = check("set terminal callback userdata", unsafe {
            sys::ghostty_terminal_set(
                terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
                userdata,
            )
        }) {
            unsafe { sys::ghostty_terminal_free(terminal) };
            return Err(error);
        }

        #[allow(
            clippy::fn_to_numeric_cast_any,
            reason = "Ghostty's generic C option setter accepts callback pointers as void pointers"
        )]
        let callback = (write_pty_callback as *const ()).cast::<c_void>();

        if let Err(error) = check("set terminal write callback", unsafe {
            sys::ghostty_terminal_set(
                terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                callback,
            )
        }) {
            unsafe { sys::ghostty_terminal_free(terminal) };
            return Err(error);
        }

        let mut render = ptr::null_mut();
        if let Err(error) = check("create render state", unsafe {
            sys::ghostty_render_state_new(ptr::null(), &raw mut render)
        }) {
            unsafe { sys::ghostty_terminal_free(terminal) };
            return Err(error);
        }

        return Ok(Self {
            terminal,
            render,
            last_snapshot: None,
            responses,
            input_limiter: VtInputLimiter::default(),
        });
    }

    /// Feed PTY output through the bounded host policy and Ghostty's VT parser.
    ///
    /// Oversized OSC strings are ignored before crossing the FFI boundary.
    pub fn write(&mut self, bytes: &[u8]) {
        let terminal = self.terminal;
        self.input_limiter.write(bytes, |bounded| unsafe {
            sys::ghostty_terminal_vt_write(terminal, bounded.as_ptr(), bounded.len());
        });
    }

    /// Encode a typed key using the terminal's current keyboard protocol modes.
    ///
    /// The encoder is configured from the live terminal immediately before the
    /// event is encoded. The host policy fixes option-as-alt to disabled.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported function/letter values, allocation
    /// failure, or a key-encoder failure.
    pub fn encode_key(&self, key: NamedKey, modifiers: KeyModifiers) -> Result<EncodedKey> {
        return input::encode_key(self.terminal, key, modifiers);
    }

    /// Encode one cell-addressed pointer event from current terminal mouse modes.
    ///
    /// Coordinates are zero-based cells. The encoder receives the center pixel
    /// of the requested cell and the exact current grid/pixel geometry.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid geometry or a pinned encoder failure.
    pub fn encode_mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: u16,
        row: u16,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<ByteString> {
        return input::encode_mouse(
            self.terminal,
            action,
            button,
            column,
            row,
            cols,
            rows,
            pixel_width,
            pixel_height,
        );
    }

    /// Drain terminal query responses emitted synchronously by the VT engine.
    pub fn take_responses(&mut self) -> Vec<Vec<u8>> {
        return take(&mut self.responses.pending);
    }

    /// Apply captured host defaults before processing terminal output.
    ///
    /// # Errors
    ///
    /// Returns an error when the palette length is invalid or libghostty
    /// rejects a color option.
    pub fn apply_terminal_colors(&mut self, colors: &TerminalColors) -> Result<()> {
        set_color_option(
            self.terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            colors.foreground,
            "set foreground color",
        )?;
        set_color_option(
            self.terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            colors.background,
            "set background color",
        )?;

        if let Some(cursor) = colors.cursor {
            set_color_option(
                self.terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                cursor,
                "set cursor color",
            )?;
        }

        if !colors.palette.is_empty() {
            if colors.palette.len() != 256 {
                return Err(Error::TerminalOperation {
                    operation: "set terminal palette",
                    reason: format!(
                        "expected an empty or 256-color palette, got {} colors",
                        colors.palette.len()
                    ),
                });
            }

            let palette: Vec<_> = colors.palette.iter().copied().map(ffi_rgb).collect();
            check("set terminal palette", unsafe {
                sys::ghostty_terminal_set(
                    self.terminal,
                    sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                    palette.as_ptr().cast::<c_void>(),
                )
            })?;
        }

        return Ok(());
    }

    /// Resize the semantic terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions are invalid.
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        if supported_geometry_cell_count(cols, rows).is_none() {
            return Err(Error::GhosttyVt {
                operation: "validate geometry",
                result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        return check("resize terminal", unsafe {
            sys::ghostty_terminal_resize(self.terminal, cols, rows, cell_width_px, cell_height_px)
        });
    }

    /// Copy the complete current render state into stable Rust structures.
    ///
    /// # Errors
    ///
    /// Returns an error when any libghostty-vt snapshot query fails.
    pub fn snapshot(&mut self) -> Result<SemanticState> {
        let terminal = self.terminal;
        let render = self.render;
        return snapshot::Snapshot::new(terminal, render, &mut self.last_snapshot).snapshot();
    }

    /// Return a complete snapshot only when semantic content changed.
    ///
    /// Dirty bookkeeping alone does not count as semantic content. This is
    /// the efficient replay path for byte-granular indexing.
    ///
    /// # Errors
    ///
    /// Returns an error when any libghostty-vt snapshot query fails.
    pub fn snapshot_if_changed(&mut self) -> Result<Option<SemanticState>> {
        let terminal = self.terminal;
        let render = self.render;
        return snapshot::Snapshot::new(terminal, render, &mut self.last_snapshot)
            .snapshot_if_changed();
    }

    /// Format the active screen using Ghostty's own formatter.
    ///
    /// # Errors
    ///
    /// Returns an error when formatter creation or output fails.
    pub fn format(&self, format: Format) -> Result<Vec<u8>> {
        return snapshot::format(self.terminal, format);
    }

    /// Format a VT stream that reconstructs the current terminal as a replay seed.
    ///
    /// Soft-wrapped rows are unwrapped so replay can apply the seed at a
    /// different terminal width without preserving incidental line breaks. Returns `None` while
    /// the cursor has a pending wrap because that behaviorally relevant state has no lossless VT
    /// serialization.
    ///
    /// # Errors
    ///
    /// Returns an error when formatter creation, state inspection, or output fails.
    pub fn format_replay_seed(&self) -> Result<Option<Vec<u8>>> {
        return snapshot::format_replay_seed(self.terminal);
    }
}

impl Drop for GhosttyVt {
    fn drop(&mut self) {
        unsafe {
            sys::ghostty_render_state_free(self.render);
            sys::ghostty_terminal_free(self.terminal);
        }
    }
}

pub(super) fn check(operation: &'static str, result: sys::GhosttyResult) -> Result<()> {
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS {
        return Ok(());
    } else {
        return Err(Error::GhosttyVt { operation, result });
    }
}

fn configure_terminal_resource_policy(terminal: sys::GhosttyTerminal) -> Result<()> {
    let kitty_storage_bytes = 0_u64;
    check("disable Kitty graphics storage", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&raw const kitty_storage_bytes).cast::<c_void>(),
        )
    })?;

    let apc_max_bytes = MAX_APC_SEQUENCE_BYTES;
    return check("limit APC sequence buffering", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
            (&raw const apc_max_bytes).cast::<c_void>(),
        )
    });
}

fn ffi_rgb([r, g, b]: [u8; 3]) -> sys::GhosttyColorRgb {
    return sys::GhosttyColorRgb { r, g, b };
}

fn set_color_option(
    terminal: sys::GhosttyTerminal,
    option: sys::GhosttyTerminalOption,
    color: [u8; 3],
    operation: &'static str,
) -> Result<()> {
    let color = ffi_rgb(color);
    return check(operation, unsafe {
        sys::ghostty_terminal_set(terminal, option, (&raw const color).cast::<c_void>())
    });
}

unsafe extern "C" fn write_pty_callback(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || (data.is_null() && len != 0) {
        return;
    }

    let bytes = if len == 0 {
        Vec::new()
    } else {
        unsafe { slice::from_raw_parts(data, len) }.to_vec()
    };

    let responses = unsafe { &mut *userdata.cast::<ResponseState>() };
    responses.pending.push(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty_vt::Format;
    use termtrace_model::terminal::state::MAX_TERMINAL_COLUMNS;

    #[test]
    fn rejects_impractical_terminal_geometry_before_ffi_allocation() {
        assert!(supported_geometry_cell_count(MAX_TERMINAL_COLUMNS.saturating_add(1), 1).is_none());
        assert!(supported_geometry_cell_count(501, 500).is_none());
        assert!(GhosttyVt::new(MAX_TERMINAL_COLUMNS.saturating_add(1), 1, 0).is_err());

        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        assert!(terminal
            .resize(MAX_TERMINAL_COLUMNS.saturating_add(1), 1, 1, 1)
            .is_err());
    }

    #[test]
    fn bounds_osc_strings_before_the_ffi_boundary() {
        let mut limiter = VtInputLimiter::new(12);
        let mut forwarded = Vec::new();
        for chunk in [
            b"before\x1b]52;0123456789abcdef".as_slice(),
            b"\x07after\x1b".as_slice(),
            b"]2;ok\x07".as_slice(),
        ] {
            limiter.write(chunk, |bytes| forwarded.extend_from_slice(bytes));
        }

        assert_eq!(forwarded, b"before\x18after\x1b]2;ok\x07");
    }

    #[test]
    fn preserves_utf8_continuations_that_match_c1_controls() {
        let mut limiter = VtInputLimiter::new(12);
        let mut forwarded = Vec::new();
        for byte in "ѝ".as_bytes() {
            limiter.write(slice::from_ref(byte), |bytes| {
                forwarded.extend_from_slice(bytes);
            });
        }

        assert_eq!(forwarded, "ѝ".as_bytes());
    }

    #[test]
    fn applies_embedder_resource_policy_to_ghostty() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        let mut kitty_storage_bytes = u64::MAX;
        check("read Kitty graphics storage limit", unsafe {
            sys::ghostty_terminal_get(
                terminal.terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw mut kitty_storage_bytes).cast::<c_void>(),
            )
        })
        .expect("read Kitty storage policy");

        assert_eq!(kitty_storage_bytes, 0);

        terminal.write(b"\x1b");
        terminal.write(b"]2;bounded\x07");
        assert_eq!(
            terminal
                .snapshot()
                .expect("snapshot")
                .metadata
                .title
                .as_bytes(),
            b"bounded"
        );
    }

    #[test]
    fn replay_seed_refuses_unserialized_pending_wrap_state() {
        let mut terminal = GhosttyVt::new(2, 2, 0).expect("create terminal");
        assert!(terminal
            .format_replay_seed()
            .expect("initial replay seed")
            .is_some());

        terminal.write(b"ab");
        assert!(terminal
            .format_replay_seed()
            .expect("pending-wrap replay seed")
            .is_none());

        terminal.write(b"c");
        assert!(terminal
            .format_replay_seed()
            .expect("post-wrap replay seed")
            .is_some());
    }

    #[test]
    fn oversized_osc_cancels_partial_parser_state() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.input_limiter = VtInputLimiter::new(12);
        terminal.write(b"\x1b(");
        terminal.write(b"\x1b]52;0123456789abcdef\x07ok");
        let plain = terminal.format(Format::Plain).expect("plain format");

        assert!(plain.windows(2).any(|window| return window == b"ok"));
    }

    #[test]
    fn emits_terminal_query_responses() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.write(b"\x1b[3;5H\x1b[6n");
        assert_eq!(terminal.take_responses(), vec![b"\x1b[3;5R".to_vec()]);
        assert!(terminal.take_responses().is_empty());
    }
}
