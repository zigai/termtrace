//! Terminal-handle lifecycle, bounded VT ingestion, and host resource policy.

use super::{
    input, snapshot,
    state::{
        supported_geometry_cell_count, EncodedKey, Format, KeyModifiers, MouseAction, MouseButton,
        NamedKey, SemanticState,
    },
    sys,
};
use crate::{Error, Result};
use std::{ffi::c_void, mem::take, ptr, slice};
use termtrace_model::execution::event::TerminalColors;
use termtrace_model::terminal::effect::TerminalEffect;
use termtrace_model::value::ByteString;

// Keep allocating control strings below the same ceiling as explanation framing.
const MAX_OSC_SEQUENCE_BYTES: usize = 1024 * 1024;
const MAX_APC_SEQUENCE_BYTES: usize = 1024 * 1024;
// Unfinished input that checkpoints must carry is bounded by the largest buffered control.
const MAX_CONTINUATION_BYTES: usize = MAX_APC_SEQUENCE_BYTES;
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const GROUND: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_GROUND.cast_unsigned();

const OSC_START: u32 =
    sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_OSC_START.cast_unsigned();

const OSC_END: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_OSC_END.cast_unsigned();

mod checkpoint;
mod effects;
mod hold_probe;

use hold_probe::HoldProbe;

/// Owned semantic engine and render snapshot state.
pub struct GhosttyVt {
    terminal: sys::GhosttyTerminal,
    effects: Box<Effects>,
    sequence: Sequence,
    hold_probe: HoldProbe,
    render_holds: bool,
    captures_effects: bool,
    processing_error: bool,
}

/// Bytes consumed by one [`GhosttyVt::write_step`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputStep {
    /// Input bytes consumed, always at least one for non-empty input.
    pub consumed: usize,
    /// Leading consumed bytes that were text outside any control sequence.
    pub plain: usize,
    /// The control sequence in the consumed bytes has not ended yet.
    pub open: bool,
    /// The step stopped just before the final byte of a sequence that begins a
    /// synchronized-output hold, so the screen is the frame the hold keeps.
    pub hold_edge: bool,
    /// The step stopped just before the final byte of a sequence that ends a
    /// synchronized-output hold, so the screen is the last frame the hold hid.
    pub release_edge: bool,
}

/// A synchronized-output (DEC mode 2026) hold change reported by Ghostty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderHold {
    /// A hold began, with the frame the program left on screen at that moment.
    Began(Box<SemanticState>),
    /// The hold ended through DECRST 2026, a full reset, or a resize.
    Ended,
}

// Shared with Ghostty callbacks, which run synchronously inside VT writes.
struct Effects {
    render: sys::GhosttyRenderState,
    last_snapshot: Option<SemanticState>,
    unmarked_rows: bool,
    responses: Vec<Vec<u8>>,
    holds: Vec<RenderHold>,
    hold_error: Option<Error>,
    reported: Vec<TerminalEffect>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Sequence {
    #[default]
    Ground,
    Open,
    Osc {
        length: usize,
    },
    DiscardOsc {
        escape: bool,
    },
}

impl Sequence {
    fn osc(introducer: sys::GhosttyTerminalStringIntroducer) -> Self {
        // The cap counts a 7-bit introducer's ESC, which the stop excludes.
        let length = if introducer
            == sys::GhosttyTerminalStringIntroducer_GHOSTTY_TERMINAL_STRING_INTRODUCER_ESC
        {
            2
        } else {
            1
        };

        return Self::Osc { length };
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Callbacks {
    render_holds: bool,
    effects: bool,
}

struct WriteStop {
    consumed: usize,
    reached: u32,
    introducer: sys::GhosttyTerminalStringIntroducer,
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
            sys::ghostty_terminal_new(ptr::null(), &raw mut terminal, cols, rows)
        })?;

        return Self::from_terminal(terminal, max_scrollback, Callbacks::default());
    }

    fn from_terminal(
        terminal: sys::GhosttyTerminal,
        max_scrollback: usize,
        callbacks: Callbacks,
    ) -> Result<Self> {
        if let Err(error) = configure_terminal_resource_policy(terminal, max_scrollback) {
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

        // Dropping the engine frees both handles from here on.
        let mut engine = Self {
            terminal,
            effects: Box::new(Effects {
                render,
                last_snapshot: None,
                unmarked_rows: false,
                responses: Vec::new(),
                holds: Vec::new(),
                hold_error: None,
                reported: Vec::new(),
            }),
            sequence: Sequence::Ground,
            hold_probe: HoldProbe::default(),
            render_holds: false,
            captures_effects: false,
            processing_error: false,
        };
        let userdata = (&raw mut *engine.effects).cast::<c_void>();

        check("set terminal callback userdata", unsafe {
            sys::ghostty_terminal_set(
                terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
                userdata,
            )
        })?;

        #[allow(
            clippy::fn_to_numeric_cast_any,
            reason = "Ghostty's generic C option setter accepts callback pointers as void pointers"
        )]
        let callback = (write_pty_callback as *const ()).cast::<c_void>();

        check("set terminal write callback", unsafe {
            sys::ghostty_terminal_set(
                terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                callback,
            )
        })?;

        if callbacks.render_holds {
            engine.capture_render_holds()?;
        }

        if callbacks.effects {
            engine.capture_effects()?;
        }

        return Ok(engine);
    }

    /// Feed PTY output through Ghostty's VT parser under the OSC size policy.
    ///
    /// Unlike [`Self::write_step`], this does not stop before synchronized-output holds.
    pub fn write(&mut self, mut bytes: &[u8]) {
        if !bytes.is_empty() {
            self.hold_probe.unknown();
        }

        while !bytes.is_empty() {
            let consumed = match self.sequence {
                Sequence::Ground | Sequence::Open => {
                    let stop = self.write_until(bytes, OSC_START);

                    // Without asking for ground, the parser may or may not be inside a sequence.
                    self.sequence = if stop.reached & OSC_START != 0 {
                        Sequence::osc(stop.introducer)
                    } else {
                        Sequence::Open
                    };

                    stop.consumed
                }
                Sequence::Osc { length } => self.write_osc(bytes, length),
                Sequence::DiscardOsc { escape } => self.discard_osc(bytes, escape),
            };
            bytes = bytes.get(consumed..).unwrap_or_default();
        }
    }

    /// Feed text up to and including the end of the next control sequence.
    ///
    /// Ghostty's own parser decides where the sequence ends, so query
    /// responses and render holds taken after the step belong to it. A
    /// sequence split across calls continues in the next call. An OSC longer
    /// than 1 MiB is cancelled, and the rest of it is dropped.
    ///
    /// While effects are captured, a bell in text also ends a step. While render
    /// holds are captured, a step also stops just before the final byte of a CSI
    /// that changes mode 2026, and reports [`OutputStep::hold_edge`] or
    /// [`OutputStep::release_edge`].
    pub fn write_step(&mut self, bytes: &[u8]) -> OutputStep {
        let mut plain = 0;
        let mut consumed = 0;
        while let Some(rest) = bytes.get(consumed..)
            && !rest.is_empty()
        {
            let written = match self.sequence {
                Sequence::Ground => {
                    // A bell in text is an effect of its own byte, so a captured one ends the step.
                    if self.captures_effects
                        && let Some(bell) = memchr::memchr2(ESC, BEL, rest)
                            .filter(|index| return rest.get(*index) == Some(&BEL))
                    {
                        let text = bell.saturating_add(1);
                        self.vt_write(rest.get(..text).unwrap_or_default());

                        return OutputStep {
                            consumed: text,
                            plain: text,
                            open: false,
                            hold_edge: false,
                            release_edge: false,
                        };
                    }

                    // C1 bytes at ground are UTF-8 to Ghostty, so only ESC starts a sequence.
                    let Some(escape) = memchr::memchr(ESC, rest) else {
                        self.vt_write(rest);

                        return OutputStep {
                            consumed: bytes.len(),
                            plain: bytes.len(),
                            open: false,
                            hold_edge: false,
                            release_edge: false,
                        };
                    };
                    self.vt_write(rest.get(..=escape).unwrap_or_default());
                    plain = escape;
                    self.sequence = Sequence::Open;
                    self.hold_probe.after_escape();

                    escape.saturating_add(1)
                }
                Sequence::Open => {
                    // Only a change of mode 2026 is an edge; Ghostty ignores repeats.
                    let held = self
                        .render_holds
                        .then(|| return self.synchronized_output().ok())
                        .flatten();

                    let hold_final = held
                        .and_then(|held| return self.hold_probe.hold_final(rest, held))
                        .filter(|index| return *index > 0);

                    let target =
                        hold_final.map_or(rest, |end| return rest.get(..end).unwrap_or_default());
                    let stop = self.write_until(target, GROUND | OSC_START);
                    self.hold_probe
                        .advance(rest.get(..stop.consumed).unwrap_or_default());

                    if stop.reached & GROUND != 0 {
                        self.sequence = Sequence::Ground;
                    } else if stop.reached & OSC_START != 0 {
                        self.sequence = Sequence::osc(stop.introducer);
                    } else if hold_final == Some(stop.consumed) {
                        return OutputStep {
                            consumed: consumed.saturating_add(stop.consumed),
                            plain,
                            open: true,
                            hold_edge: held == Some(false),
                            release_edge: held == Some(true),
                        };
                    }

                    stop.consumed
                }
                Sequence::Osc { length } => {
                    let written = self.write_osc(rest, length);
                    self.sync_probe_after_string(rest, written);

                    written
                }
                Sequence::DiscardOsc { escape } => {
                    let written = self.discard_osc(rest, escape);
                    self.sync_probe_after_string(rest, written);

                    written
                }
            };
            consumed = consumed.saturating_add(written);

            if self.sequence == Sequence::Ground {
                self.hold_probe.reset();
            }

            if self.sequence == Sequence::Ground && consumed > plain {
                break;
            }
        }

        return OutputStep {
            consumed,
            plain,
            open: self.sequence != Sequence::Ground,
            hold_edge: false,
            release_edge: false,
        };
    }

    // An OSC ends at the ESC of ST, which may instead begin another sequence.
    fn sync_probe_after_string(&mut self, bytes: &[u8], written: usize) {
        if self.sequence == Sequence::Open {
            if written > 0 && bytes.get(written - 1) == Some(&ESC) {
                self.hold_probe.after_escape();
            } else {
                self.hold_probe.unknown();
            }
        }
    }

    /// Report synchronized-output holds through [`Self::take_render_holds`].
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt rejects the callback.
    pub fn capture_render_holds(&mut self) -> Result<()> {
        #[allow(
            clippy::fn_to_numeric_cast_any,
            reason = "Ghostty's generic C option setter accepts callback pointers as void pointers"
        )]
        let callback = (render_hold_callback as *const ()).cast::<c_void>();

        check("set render hold callback", unsafe {
            sys::ghostty_terminal_set(
                self.terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
                callback,
            )
        })?;

        self.render_holds = true;
        return Ok(());
    }

    /// Report side effects such as bells, titles, OSC 133 marks, and ignored
    /// control strings through [`Self::take_effects`].
    ///
    /// Observation only: no query is answered that was not answered before.
    /// Kitty clipboard (OSC 5522) writes are still refused with ENOSYS, but
    /// when they complete rather than when they begin.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt rejects a callback.
    pub fn capture_effects(&mut self) -> Result<()> {
        effects::register(self.terminal)?;
        self.captures_effects = true;
        return Ok(());
    }

    /// Drain effects reported since the last call, oldest first.
    ///
    /// Ghostty reports an effect when the byte that completes its sequence is
    /// written, so effects drained after a [`Self::write_step`] belong to the
    /// step's last byte. The first time Ghostty records a processing error,
    /// [`TerminalEffect::ProcessingError`] follows the step's other effects.
    pub fn take_effects(&mut self) -> Vec<TerminalEffect> {
        let mut effects = take(&mut self.effects.reported);

        if self.captures_effects && !self.processing_error && self.ghostty_processing_error() {
            self.processing_error = true;
            effects.push(TerminalEffect::ProcessingError);
        }

        return effects;
    }

    fn ghostty_processing_error(&self) -> bool {
        let mut error = false;
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_VT_PROCESSING_ERROR,
                (&raw mut error).cast::<c_void>(),
            )
        };

        return result == sys::GhosttyResult_GHOSTTY_SUCCESS && error;
    }

    /// Drain render holds reported since the last call, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when capturing the frame at the start of a hold failed.
    pub fn take_render_holds(&mut self) -> Result<Vec<RenderHold>> {
        let holds = take(&mut self.effects.holds);

        if let Some(error) = self.effects.hold_error.take() {
            return Err(error);
        }
        return Ok(holds);
    }

    fn write_osc(&mut self, bytes: &[u8], length: usize) -> usize {
        let budget = MAX_OSC_SEQUENCE_BYTES.saturating_sub(length);
        let mut limit = bytes.len().min(budget);

        // Ghostty ends an OSC at the ESC of ST, but the byte after it would exceed the cap.
        if limit == budget && bytes.get(limit.saturating_sub(1)) == Some(&ESC) {
            limit = limit.saturating_sub(1);
        }

        if limit == 0 {
            // CAN makes Ghostty discard the partial OSC without running it.
            self.vt_write(&[CAN]);
            self.sequence = Sequence::DiscardOsc { escape: false };
            return 0;
        }

        let stop = self.write_until(bytes.get(..limit).unwrap_or_default(), GROUND | OSC_END);
        self.sequence = if stop.reached & GROUND != 0 {
            Sequence::Ground
        } else if stop.reached & OSC_END != 0 {
            Sequence::Open
        } else {
            Sequence::Osc {
                length: length.saturating_add(stop.consumed),
            }
        };

        return stop.consumed;
    }

    fn discard_osc(&mut self, bytes: &[u8], mut escape: bool) -> usize {
        for (index, &byte) in bytes.iter().enumerate() {
            if matches!(byte, 0x07 | 0x9c | CAN | 0x1a) || (escape && byte == b'\\') {
                self.sequence = Sequence::Ground;

                return index.saturating_add(1);
            }

            escape = byte == ESC;
        }

        self.sequence = Sequence::DiscardOsc { escape };

        return bytes.len();
    }

    fn vt_write(&mut self, bytes: &[u8]) {
        self.note_unmarked_rows(bytes);
        unsafe { sys::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    // Ghostty can attach a zero-width codepoint to a cell without marking its row dirty, and
    // only non-ASCII input can complete such a codepoint.
    fn note_unmarked_rows(&mut self, bytes: &[u8]) {
        self.effects.unmarked_rows |= !bytes.is_ascii();
    }

    fn write_until(&mut self, bytes: &[u8], boundaries: u32) -> WriteStop {
        self.note_unmarked_rows(bytes);

        let mut stop = sys::GhosttyTerminalWriteStop {
            size: size_of::<sys::GhosttyTerminalWriteStop>(),
            ..sys::GhosttyTerminalWriteStop::default()
        };
        let result = unsafe {
            sys::ghostty_terminal_vt_write_until(
                self.terminal,
                bytes.as_ptr(),
                bytes.len(),
                boundaries,
                &raw mut stop,
            )
        };

        // Ghostty rejects only null or malformed arguments, which are never passed here.
        if result != sys::GhosttyResult_GHOSTTY_SUCCESS {
            return WriteStop {
                consumed: bytes.len(),
                reached: 0,
                introducer: 0,
            };
        }
        return WriteStop {
            consumed: stop.consumed,
            reached: stop.reached,
            introducer: stop.introducer,
        };
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
        return take(&mut self.effects.responses);
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

    /// Set the terminfo name reported for XTGETTCAP `TN` queries.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty rejects the name, such as one longer
    /// than 128 bytes.
    pub fn set_terminfo_name(&mut self, name: &str) -> Result<()> {
        let name = sys::GhosttyString {
            ptr: name.as_ptr(),
            len: name.len(),
        };

        return check("set terminfo name", unsafe {
            sys::ghostty_terminal_set(
                self.terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
                (&raw const name).cast::<c_void>(),
            )
        });
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
        let effects = &mut *self.effects;

        return snapshot::Snapshot::new(
            self.terminal,
            &mut effects.render,
            &mut effects.last_snapshot,
            &mut effects.unmarked_rows,
        )
        .snapshot();
    }

    /// Whether synchronized output (DEC private mode 2026) is enabled, without a snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot report the mode.
    pub fn synchronized_output(&self) -> Result<bool> {
        return snapshot::terminal_mode(self.terminal, 2026, false);
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
        let effects = &mut *self.effects;

        return snapshot::Snapshot::new(
            self.terminal,
            &mut effects.render,
            &mut effects.last_snapshot,
            &mut effects.unmarked_rows,
        )
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
}

impl Drop for GhosttyVt {
    fn drop(&mut self) {
        unsafe {
            sys::ghostty_render_state_free(self.effects.render);

            sys::ghostty_terminal_free(self.terminal);
        }
    }
}

pub(super) fn check(operation: &'static str, result: sys::GhosttyResult) -> Result<()> {
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS {
        return Ok(());
    }
    return Err(Error::GhosttyVt { operation, result });
}

fn configure_terminal_resource_policy(
    terminal: sys::GhosttyTerminal,
    max_scrollback_bytes: usize,
) -> Result<()> {
    check("limit scrollback bytes", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
            (&raw const max_scrollback_bytes).cast::<c_void>(),
        )
    })?;

    let kitty_storage_bytes = 0_u64;
    check("disable Kitty graphics storage", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&raw const kitty_storage_bytes).cast::<c_void>(),
        )
    })?;

    let apc_max_bytes = MAX_APC_SEQUENCE_BYTES;
    check("limit APC sequence buffering", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
            (&raw const apc_max_bytes).cast::<c_void>(),
        )
    })?;

    let continuation_max_bytes = MAX_CONTINUATION_BYTES;

    return check("track unfinished VT input", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
            (&raw const continuation_max_bytes).cast::<c_void>(),
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

    let effects = unsafe { &mut *userdata.cast::<Effects>() };
    effects.responses.push(bytes);
}

unsafe extern "C" fn render_hold_callback(
    terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    held: bool,
) {
    if userdata.is_null() {
        return;
    }

    let effects = unsafe { &mut *userdata.cast::<Effects>() };

    if !held {
        effects.holds.push(RenderHold::Ended);

        return;
    }

    match snapshot::Snapshot::new(
        terminal,
        &mut effects.render,
        &mut effects.last_snapshot,
        &mut effects.unmarked_rows,
    )
    .snapshot()
    {
        Ok(mut frame) => {
            // Ghostty sets mode 2026 before reporting the hold; the kept frame predates it.
            frame.modes.synchronized_output = false;

            for mode in &mut frame.modes.values {
                if mode.number == 2026 && !mode.ansi {
                    mode.enabled = false;
                }
            }

            effects.holds.push(RenderHold::Began(Box::new(frame)));
        }
        Err(error) => {
            effects.hold_error.get_or_insert(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty_vt::Format;
    use termtrace_model::terminal::{
        effect::{
            ClipboardRepresentation, ClipboardTarget, PayloadDigest, ProgressState, PromptKind,
            UnknownSequenceKind,
        },
        state::MAX_TERMINAL_COLUMNS,
    };

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
    fn emits_terminal_query_responses() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.write(b"\x1b[3;5H\x1b[6n");
        assert_eq!(terminal.take_responses(), vec![b"\x1b[3;5R".to_vec()]);
        assert!(terminal.take_responses().is_empty());
    }

    #[test]
    fn write_steps_end_after_one_sequence_as_ghostty_parses_it() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        let input = b"ab\x1b[31mcd\x1b]2;t\x1b\\\x9b6n\xc4\x9b";

        let mut steps = Vec::new();
        let mut offset = 0;

        while offset < input.len() {
            let step = terminal.write_step(&input[offset..]);
            steps.push(step);
            offset += step.consumed;
        }

        let step = |consumed, plain| {
            return OutputStep {
                consumed,
                plain,
                open: false,
                hold_edge: false,
                release_edge: false,
            };
        };

        assert_eq!(steps, vec![step(7, 2), step(9, 2), step(5, 5)]);
        assert!(terminal.take_responses().is_empty());
        assert_eq!(
            terminal
                .snapshot()
                .expect("snapshot")
                .metadata
                .title
                .as_bytes(),
            b"t"
        );

        let open = terminal.write_step(b"\x1b[6");
        assert_eq!(
            open,
            OutputStep {
                consumed: 3,
                plain: 0,
                open: true,
                hold_edge: false,
                release_edge: false,
            }
        );
        assert!(terminal.take_responses().is_empty());
        assert_eq!(terminal.write_step(b"nz"), step(1, 0));
        assert_eq!(terminal.take_responses().len(), 1);
    }

    #[test]
    fn drops_the_rest_of_an_oversized_osc_until_its_terminator() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        let filler = vec![b'a'; MAX_OSC_SEQUENCE_BYTES];

        terminal.write(b"\x1b]2;");

        for chunk in filler.chunks(4096) {
            terminal.write(chunk);
        }

        // Ghostty keeps 0x9C as OSC content; only the dropped tail ends at it.
        terminal.write(b"\x9cvisible\x1b]2;bounded\x07");
        let state = terminal.snapshot().expect("snapshot");
        assert_eq!(state.metadata.title.as_bytes(), b"bounded");

        let plain = terminal.format(Format::Plain).expect("plain format");
        assert!(plain.starts_with(b"visible"));
    }

    fn effects_of(chunks: &[&[u8]]) -> Vec<TerminalEffect> {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.capture_effects().expect("capture effects");

        let mut effects = Vec::new();

        for chunk in chunks {
            let mut offset = 0;

            while offset < chunk.len() {
                offset += terminal.write_step(&chunk[offset..]).consumed;
                effects.extend(terminal.take_effects());
            }
        }

        return effects;
    }

    #[test]
    fn reports_shell_integration_marks_with_and_without_exit_codes() {
        let effects = effects_of(&[
            b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C;cmdline_url=ls%20-l\x07",
            b"out\r\n\x1b]133;D;2\x07\x1b]133;A;k=s\x1b\\\x1b]133;D\x07",
        ]);
        assert_eq!(
            effects,
            vec![
                TerminalEffect::PromptStart {
                    prompt: PromptKind::Primary
                },
                TerminalEffect::InputStart,
                TerminalEffect::CommandStart {
                    command: Some(ByteString::from(&b"ls -l"[..]))
                },
                TerminalEffect::CommandEnd {
                    exit_code: Some(2),
                    error: None
                },
                TerminalEffect::PromptStart {
                    prompt: PromptKind::Secondary
                },
                TerminalEffect::CommandEnd {
                    exit_code: None,
                    error: None
                },
            ]
        );
    }

    #[test]
    fn a_bel_terminating_an_osc_is_not_a_bell() {
        let effects = effects_of(&[b"\x1b]2;title\x07\x07\x1b]2;", b"split\x07"]);
        assert_eq!(
            effects,
            vec![
                TerminalEffect::TitleChanged {
                    title: ByteString::from(&b"title"[..])
                },
                TerminalEffect::Bell,
                TerminalEffect::TitleChanged {
                    title: ByteString::from(&b"split"[..])
                },
            ]
        );
    }

    #[test]
    fn effects_split_across_writes_are_reported_once_when_their_sequence_completes() {
        let input = b"\x1b]7;file://host/tmp\x1b\\\x1b]9;4;1;40\x07\x1b[8;30;100t\x1bc";
        let whole = effects_of(&[input]);
        let bytes = input.iter().map(slice::from_ref).collect::<Vec<_>>();
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.capture_effects().expect("capture effects");

        let mut completing = Vec::new();
        for (offset, byte) in bytes.iter().enumerate() {
            terminal.write(byte);
            completing.extend(terminal.take_effects().into_iter().map(|_| return offset));
        }

        assert_eq!(
            whole,
            vec![
                TerminalEffect::WorkingDirectoryChanged {
                    location: ByteString::from(&b"file://host/tmp"[..])
                },
                TerminalEffect::ProgressReport {
                    state: ProgressState::Set,
                    percent: Some(40)
                },
                TerminalEffect::WindowResizeRequest {
                    rows: Some(30),
                    columns: Some(100)
                },
                // A full reset clears progress before Ghostty reports the reset itself.
                TerminalEffect::ProgressReport {
                    state: ProgressState::Remove,
                    percent: None
                },
                TerminalEffect::FullReset,
            ]
        );
        assert_eq!(effects_of(&bytes), whole);
        // Ghostty completes an OSC at the ESC of its ST, before the backslash.
        assert_eq!(completing, vec![19, 31, 42, 44, 44]);
    }

    #[test]
    fn clipboard_and_notification_payloads_are_only_digested() {
        let effects = effects_of(&[b"\x1b]52;c;c2VjcmV0\x07\x1b]777;notify;Title;hunter2\x07"]);
        assert_eq!(
            effects,
            vec![
                TerminalEffect::ClipboardWrite {
                    target: ClipboardTarget::Standard,
                    representations: vec![ClipboardRepresentation {
                        mime: "text/plain".to_owned(),
                        data: PayloadDigest::of(b"secret"),
                    }],
                },
                TerminalEffect::DesktopNotification {
                    title: PayloadDigest::of(b"Title"),
                    body: PayloadDigest::of(b"hunter2"),
                },
            ]
        );

        let json = serde_json::to_string(&effects).expect("serialize effects");
        assert!(!json.contains("secret") && !json.contains("c2VjcmV0"));
        assert!(!json.contains("hunter2") && !json.contains("aHVudGVyMg"));
    }

    #[test]
    fn unknown_control_strings_keep_only_a_bounded_identifier() {
        let long_parameter = [b'x'; 8192];
        let mut osc = b"\x1b]7400;token=abc".to_vec();
        osc.extend_from_slice(&long_parameter);
        osc.push(0x07);
        let effects = effects_of(&[
            &osc,
            b"\x1b_Zcustom;secret\x1b\\",
            b"\x1b_abcdefghijklmnopqrstuvwxyz\x1b\\",
        ]);

        let identifiers = effects
            .iter()
            .map(|effect| match effect {
                TerminalEffect::UnknownSequence { kind, identifier } => {
                    return (*kind, identifier.as_bytes().to_vec());
                }
                other => panic!("expected an unknown sequence, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            identifiers,
            vec![
                (UnknownSequenceKind::Osc, b"7400".to_vec()),
                (UnknownSequenceKind::Apc, b"Zcustom".to_vec()),
                (UnknownSequenceKind::Apc, b"abcdefghijklmnop".to_vec()),
            ]
        );
    }

    #[test]
    fn engines_without_effect_capture_report_nothing_and_answer_as_before() {
        let input = b"\x07\x1b]2;t\x07\x1b]52;c;?\x07\x1b]7501;?\x07\x1b[6n";
        let mut plain = GhosttyVt::new(80, 24, 0).expect("create terminal");
        let mut capturing = GhosttyVt::new(80, 24, 0).expect("create terminal");
        capturing.capture_effects().expect("capture effects");

        plain.write(input);
        capturing.write(input);

        assert!(plain.take_effects().is_empty());
        assert_eq!(capturing.take_effects().len(), 2);
        assert_eq!(plain.take_responses(), capturing.take_responses());
    }

    #[test]
    fn reports_render_holds_with_the_frame_left_on_screen() {
        let mut terminal = GhosttyVt::new(20, 4, 0).expect("create terminal");
        terminal.capture_render_holds().expect("capture holds");
        terminal.write(b"shown");
        terminal.write(b"\x1b[?2026;1049h");
        let holds = terminal.take_render_holds().expect("hold start");
        let [RenderHold::Began(frame)] = holds.as_slice() else {
            panic!("expected one hold start, got {holds:?}");
        };

        assert!(!frame.modes.synchronized_output);
        assert!(!frame.modes.alternate_screen);
        assert_eq!(frame.grid[0].cells[0].grapheme, "s");
        assert!(terminal.synchronized_output().expect("mode"));

        terminal.write(b"\x1b[?2026h\x1b[?2026l\x1b[?2026l");
        assert_eq!(
            terminal.take_render_holds().expect("hold end"),
            vec![RenderHold::Ended]
        );

        terminal.write(b"\x1b[?2026h");
        terminal.resize(30, 4, 1, 1).expect("resize");
        terminal.write(b"\x1b[?2026h\x1bc");
        let holds = terminal.take_render_holds().expect("resize and reset");
        assert!(matches!(
            holds.as_slice(),
            [
                RenderHold::Began(_),
                RenderHold::Ended,
                RenderHold::Began(_),
                RenderHold::Ended
            ]
        ));
        assert!(!terminal.synchronized_output().expect("mode"));
    }

    fn steps(terminal: &mut GhosttyVt, input: &[u8]) -> Vec<OutputStep> {
        let mut steps = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            let step = terminal.write_step(&input[offset..]);
            assert!(step.consumed > 0 && step.consumed <= input.len() - offset);
            steps.push(step);
            offset += step.consumed;
        }

        return steps;
    }

    #[test]
    fn steps_stop_before_the_byte_that_begins_a_hold() {
        let mut terminal = GhosttyVt::new(20, 4, 0).expect("create terminal");
        terminal.capture_render_holds().expect("capture holds");
        let first = terminal.write_step(b"ab[?1049;2026hc");
        assert_eq!((first.consumed, first.hold_edge), (14, true));

        let edge = terminal.snapshot().expect("edge");
        assert!(!edge.modes.synchronized_output);
        assert!(!edge.modes.alternate_screen);
        assert_eq!(edge.grid[0].cells[1].grapheme, "b");
        assert!(terminal
            .take_render_holds()
            .expect("no hold yet")
            .is_empty());

        let last = terminal.write_step(b"hc");
        assert_eq!((last.consumed, last.hold_edge), (1, false));
        assert!(terminal.synchronized_output().expect("mode"));
        assert_eq!(terminal.take_render_holds().expect("hold").len(), 1);
    }

    #[test]
    fn hold_edges_follow_sequences_across_writes_and_inline_controls() {
        let mut terminal = GhosttyVt::new(20, 4, 0).expect("create terminal");
        terminal.capture_render_holds().expect("capture holds");
        assert!(!terminal.write_step(b"\x1b[?20").hold_edge);

        let resumed = terminal.write_step(b"26\nh");
        assert_eq!((resumed.consumed, resumed.hold_edge), (3, true));
        assert_eq!(terminal.snapshot().expect("edge").cursor.row, Some(1));
        assert!(!terminal.synchronized_output().expect("mode"));

        // A sequence whose final byte starts the next write has no earlier byte to stop at.
        steps(&mut terminal, b"h\x1b[?2026l\x1b[?2026");
        assert!(!terminal.write_step(b"h").hold_edge);
        assert!(terminal.synchronized_output().expect("mode"));

        steps(&mut terminal, b"\x1b[?2026l\x1b[?20");
        let checkpoint = terminal
            .encode_checkpoint()
            .expect("encode")
            .expect("checkpoint");

        let mut restored = GhosttyVt::restore_checkpoint(&checkpoint, 0).expect("restore");
        assert!(restored.write_step(b"26h").hold_edge);
    }

    #[test]
    fn a_string_before_a_hold_reaches_the_engine_whole_at_every_split() {
        for input in [
            &b"before\x1b]0;title\x07\x1b[?2026hhidden\x1b[?2026lafter"[..],
            &b"before\x1b]2;title\x1b\\\x1b[?2026hhidden\x1b[?2026lafter"[..],
        ] {
            let mut whole = GhosttyVt::new(20, 4, 0).expect("create terminal");
            whole.capture_render_holds().expect("capture holds");
            whole.capture_effects().expect("capture effects");
            whole.write(input);
            let expected = whole.snapshot().expect("whole state");
            let expected_holds = whole.take_render_holds().expect("whole holds");
            let expected_effects = whole.take_effects();
            assert_eq!(expected.metadata.title.as_bytes(), b"title");
            assert_eq!(
                expected.grid[0].cells[..17]
                    .iter()
                    .map(|cell| return cell.grapheme.as_str())
                    .collect::<String>(),
                "beforehiddenafter"
            );
            assert_eq!(
                expected_effects,
                vec![TerminalEffect::TitleChanged {
                    title: ByteString::from(&b"title"[..]),
                }]
            );
            assert!(matches!(
                expected_holds.as_slice(),
                [RenderHold::Began(_), RenderHold::Ended]
            ));

            // Include an unsplit write and every cut inside the OSC, its terminator,
            // and the mode sequences. A look-ahead hold must not skip an OSC stop.
            for split in 0..=input.len() {
                for stepped in [false, true] {
                    let mut terminal = GhosttyVt::new(20, 4, 0).expect("create terminal");
                    terminal.capture_render_holds().expect("capture holds");
                    terminal.capture_effects().expect("capture effects");

                    for chunk in [&input[..split], &input[split..]] {
                        if stepped {
                            steps(&mut terminal, chunk);
                        } else {
                            terminal.write(chunk);
                        }
                    }

                    assert_eq!(
                        terminal.snapshot().expect("split state"),
                        expected,
                        "split={split}, stepped={stepped}"
                    );
                    assert_eq!(
                        terminal.take_effects(),
                        expected_effects,
                        "split={split}, stepped={stepped}"
                    );
                    assert_eq!(
                        terminal.take_render_holds().expect("split holds"),
                        expected_holds,
                        "split={split}, stepped={stepped}"
                    );
                }
            }
        }
    }

    #[test]
    fn enabling_synchronized_output_again_inside_a_hold_is_not_an_edge_at_any_split() {
        let repeat = b"\x1b[?2026hmore";

        for split in 0..=repeat.len() {
            // Also cover an existing hold established by ordinary writes: the
            // probe must consult the engine's mode rather than its own history.
            for stepped_start in [false, true] {
                let mut terminal = GhosttyVt::new(20, 4, 0).expect("create terminal");
                terminal.capture_render_holds().expect("capture holds");

                let initial = b"shown\x1b[?2026h\x1b[2J\x1b[Hhidden";

                if stepped_start {
                    steps(&mut terminal, initial);
                } else {
                    terminal.write(initial);
                }

                let holds = terminal.take_render_holds().expect("initial hold");
                let [RenderHold::Began(kept)] = holds.as_slice() else {
                    panic!("expected one hold start, got {holds:?}");
                };
                assert_eq!(
                    kept.grid[0].cells[..5]
                        .iter()
                        .map(|cell| return cell.grapheme.as_str())
                        .collect::<String>(),
                    "shown"
                );

                for chunk in [&repeat[..split], &repeat[split..]] {
                    assert!(
                        steps(&mut terminal, chunk)
                            .iter()
                            .all(|step| return !step.hold_edge && !step.release_edge),
                        "split={split}, stepped_start={stepped_start}"
                    );
                    assert!(terminal
                        .take_render_holds()
                        .expect("no new hold")
                        .is_empty());
                    assert!(terminal.synchronized_output().expect("mode"));
                }

                let hidden = terminal.snapshot().expect("hidden state");
                assert_eq!(
                    hidden.grid[0].cells[..10]
                        .iter()
                        .map(|cell| return cell.grapheme.as_str())
                        .collect::<String>(),
                    "hiddenmore"
                );
                steps(&mut terminal, b"\x1b[?2026l");
                assert_eq!(
                    terminal.take_render_holds().expect("release"),
                    vec![RenderHold::Ended]
                );
                assert!(!terminal.synchronized_output().expect("released mode"));
            }
        }
    }

    #[test]
    fn steps_do_not_stop_for_other_modes_or_without_hold_capture() {
        let mut capturing = GhosttyVt::new(20, 4, 0).expect("create terminal");
        capturing.capture_render_holds().expect("capture holds");
        let other = b"\x1b[?25h\x1b[?2026l\x1b[2026h\x1b[?2026:1h\x1b[>2026h";
        assert!(steps(&mut capturing, other)
            .iter()
            .all(|step| return !step.hold_edge));
        assert!(!capturing.synchronized_output().expect("mode"));

        let mut plain = GhosttyVt::new(20, 4, 0).expect("create terminal");
        assert!(steps(&mut plain, b"\x1b[?2026h")
            .iter()
            .all(|step| return !step.hold_edge));
        assert!(plain.synchronized_output().expect("mode"));
    }
}
