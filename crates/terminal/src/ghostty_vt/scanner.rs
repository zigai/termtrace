//! Control-sequence boundaries from Ghostty's parser, without rendering text.

use super::{sys, terminal::check};
use crate::Result;
use std::{ffi::c_void, ptr};

const ESC: u8 = 0x1b;
const SCROLLBACK_BYTES: usize = 1024;
const GROUND: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_GROUND.cast_unsigned();

const APC_START: u32 =
    sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_APC_START.cast_unsigned();

const APC_END: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_APC_END.cast_unsigned();

const DCS_START: u32 =
    sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_DCS_START.cast_unsigned();

const DCS_END: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_DCS_END.cast_unsigned();

const OSC_START: u32 =
    sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_OSC_START.cast_unsigned();

const OSC_END: u32 = sys::GhosttyTerminalBoundary_GHOSTTY_TERMINAL_BOUNDARY_OSC_END.cast_unsigned();
const STARTS: u32 = APC_START | DCS_START | OSC_START;
const ENDS: u32 = APC_END | DCS_END | OSC_END;

/// One step of [`SequenceScanner::scan`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanStep {
    /// Input bytes the step covers, always at least one for non-empty input.
    pub consumed: usize,
    /// What the covered bytes are.
    pub kind: ScanKind,
}

/// The parser's view of the bytes covered by one [`ScanStep`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanKind {
    /// Text outside any control sequence, up to but excluding the next ESC.
    Text,
    /// Bytes inside control sequences.
    Sequence {
        /// The first byte is an ESC that starts a new escape sequence.
        escape: bool,
        /// The parser returned to ground with the last byte.
        ground: bool,
        /// A control string started at the last byte.
        start: Option<StringStart>,
        /// A control string ended at the last byte.
        end: Option<StringEnd>,
    },
}

/// The start of an APC, SOS, PM, DCS, or OSC string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StringStart {
    /// Which string started.
    pub kind: ControlString,
    /// The string started with a single 8-bit introducer rather than ESC and a byte.
    pub eight_bit: bool,
}

/// A control-string family as Ghostty's parser distinguishes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlString {
    /// Application program command.
    Apc,
    /// Start of string.
    Sos,
    /// Privacy message.
    Pm,
    /// Device control string.
    Dcs,
    /// Operating system command.
    Osc,
}

/// How a control string ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringEnd {
    /// ESC, the first byte of `ESC \`; the parser is in a new escape sequence.
    Escape,
    /// The 8-bit string terminator 0x9C.
    Terminator,
    /// BEL ended an OSC.
    Bell,
    /// CAN or SUB discarded the string.
    Cancelled,
    /// Another 8-bit control byte cut the string short.
    Aborted,
}

/// A scratch libghostty-vt terminal used only for its parser.
///
/// Text outside control sequences never reaches Ghostty, which only leaves
/// ground on ESC. Control sequences are executed on a small terminal with
/// image storage disabled, so memory stays bounded by Ghostty's own
/// per-sequence limits.
pub struct SequenceScanner {
    terminal: sys::GhosttyTerminal,
    ground: bool,
}

impl SequenceScanner {
    /// Create a scanner at ground.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate or configure the terminal.
    pub fn new() -> Result<Self> {
        let mut terminal = ptr::null_mut();
        check("create scanner terminal", unsafe {
            sys::ghostty_terminal_new(ptr::null(), &raw mut terminal, 80, 24)
        })?;

        // Dropping the scanner frees the terminal from here on.
        let scanner = Self {
            terminal,
            ground: true,
        };

        // A zero limit grew RSS past 2 GiB in ASan fuzz runs; Ghostty prunes whole pages either way.
        scanner.set_size(
            "limit scanner scrollback",
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
            SCROLLBACK_BYTES,
        )?;
        scanner.set_size(
            "disable scanner APC buffering",
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
            0,
        )?;

        let kitty_storage_bytes = 0_u64;
        check("disable scanner Kitty graphics storage", unsafe {
            sys::ghostty_terminal_set(
                scanner.terminal,
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw const kitty_storage_bytes).cast::<c_void>(),
            )
        })?;
        return Ok(scanner);
    }

    /// Whether the parser is outside every control sequence.
    #[must_use]
    pub fn is_ground(&self) -> bool {
        return self.ground;
    }

    /// Advance over text, or over control-sequence bytes up to the next
    /// boundary Ghostty reports.
    ///
    /// A sequence step stops before any later ESC, so every ESC begins a step.
    pub fn scan(&mut self, bytes: &[u8]) -> ScanStep {
        let Some(&first) = bytes.first() else {
            return ScanStep {
                consumed: 0,
                kind: ScanKind::Text,
            };
        };
        if self.ground && first != ESC {
            return ScanStep {
                consumed: memchr::memchr(ESC, bytes).unwrap_or(bytes.len()),
                kind: ScanKind::Text,
            };
        }

        let limit = memchr::memchr(ESC, bytes.get(1..).unwrap_or_default())
            .map_or(bytes.len(), |escape| return escape.saturating_add(1));

        if self.ground {
            // Asking for ground at ground consumes nothing, so ESC goes in first.
            let escape = [ESC];
            unsafe { sys::ghostty_terminal_vt_write(self.terminal, escape.as_ptr(), 1) };

            self.ground = false;

            let rest = bytes.get(1..limit).unwrap_or_default();
            let mut step = self.write_until(rest);
            step.consumed = step.consumed.saturating_add(1);

            if let ScanKind::Sequence { escape, .. } = &mut step.kind {
                *escape = true;
            }

            return step;
        }

        let mut step = self.write_until(bytes.get(..limit).unwrap_or_default());
        if let ScanKind::Sequence { escape, end, .. } = &mut step.kind {
            *escape = first == ESC && *end != Some(StringEnd::Escape);
        }

        return step;
    }

    /// Like [`Self::scan`], but a sequence step runs on to ground without
    /// stopping at string boundaries or ESC, and reports no string boundaries.
    pub fn scan_to_ground(&mut self, bytes: &[u8]) -> ScanStep {
        let Some(&first) = bytes.first() else {
            return ScanStep {
                consumed: 0,
                kind: ScanKind::Text,
            };
        };

        let mut consumed = 0;

        if self.ground {
            if first != ESC {
                return ScanStep {
                    consumed: memchr::memchr(ESC, bytes).unwrap_or(bytes.len()),
                    kind: ScanKind::Text,
                };
            }

            let escape = [ESC];
            unsafe { sys::ghostty_terminal_vt_write(self.terminal, escape.as_ptr(), 1) };

            self.ground = false;
            consumed = 1;
        }

        let rest = bytes.get(consumed..).unwrap_or_default();
        let stop = self.write_stop(rest, GROUND);
        self.ground = stop.reached & GROUND != 0;

        return ScanStep {
            consumed: consumed.saturating_add(stop.consumed),
            kind: ScanKind::Sequence {
                escape: first == ESC,
                ground: self.ground,
                start: None,
                end: None,
            },
        };
    }

    /// Whether the scratch terminal shows its alternate screen.
    #[must_use]
    pub fn alternate_screen(&self) -> bool {
        let mut screen = sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&raw mut screen).cast::<c_void>(),
            )
        };

        return result == sys::GhosttyResult_GHOSTTY_SUCCESS
            && screen == sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
    }

    fn write_until(&mut self, bytes: &[u8]) -> ScanStep {
        let stop = self.write_stop(bytes, GROUND | STARTS | ENDS);
        let ground = stop.reached & GROUND != 0;
        self.ground = ground;

        let start = (stop.reached & STARTS != 0).then(|| {
            let kind = if stop.reached & OSC_START != 0 {
                ControlString::Osc
            } else if stop.reached & DCS_START != 0 {
                ControlString::Dcs
            } else {
                match stop.byte {
                    b'X' | 0x98 => ControlString::Sos,
                    b'^' | 0x9e => ControlString::Pm,
                    _ => ControlString::Apc,
                }
            };

            return StringStart {
                kind,
                eight_bit: stop.introducer
                    == sys::GhosttyTerminalStringIntroducer_GHOSTTY_TERMINAL_STRING_INTRODUCER_C1,
            };
        });

        let end = (stop.reached & ENDS != 0).then(|| {
            return match stop.end {
                sys::GhosttyTerminalStringEnd_GHOSTTY_TERMINAL_STRING_END_ST_ESC => {
                    StringEnd::Escape
                }
                sys::GhosttyTerminalStringEnd_GHOSTTY_TERMINAL_STRING_END_ST_C1 => {
                    StringEnd::Terminator
                }
                sys::GhosttyTerminalStringEnd_GHOSTTY_TERMINAL_STRING_END_CANCELLED => {
                    StringEnd::Cancelled
                }
                sys::GhosttyTerminalStringEnd_GHOSTTY_TERMINAL_STRING_END_BEL => StringEnd::Bell,
                _ => StringEnd::Aborted,
            };
        });

        return ScanStep {
            consumed: stop.consumed,
            kind: ScanKind::Sequence {
                escape: false,
                ground,
                start,
                end,
            },
        };
    }

    fn write_stop(&mut self, bytes: &[u8], boundaries: u32) -> sys::GhosttyTerminalWriteStop {
        let mut stop = sys::GhosttyTerminalWriteStop {
            size: size_of::<sys::GhosttyTerminalWriteStop>(),
            ..sys::GhosttyTerminalWriteStop::default()
        };

        if bytes.is_empty() {
            return stop;
        }

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
            stop.consumed = bytes.len();
            stop.reached = 0;
        }

        return stop;
    }

    fn set_size(
        &self,
        operation: &'static str,
        option: sys::GhosttyTerminalOption,
        value: usize,
    ) -> Result<()> {
        return check(operation, unsafe {
            sys::ghostty_terminal_set(self.terminal, option, (&raw const value).cast::<c_void>())
        });
    }
}

impl Drop for SequenceScanner {
    fn drop(&mut self) {
        unsafe { sys::ghostty_terminal_free(self.terminal) };
    }
}
