//! Safe, narrow adapter around the pinned libghostty-vt C API.

#![allow(
    unsafe_code,
    reason = "the module is the audited boundary around Ghostty's C API"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "terminal mode snapshots intentionally preserve independent protocol flags"
)]

use std::{
    ffi::c_void,
    mem::{size_of, take},
    ptr, slice,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::{ByteString, TerminalColors},
};

mod sys;

/// RGB color independent of libghostty's unstable ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

/// Cell color source before default-color fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ColorSpec {
    /// No explicit color.
    None,
    /// Index into the active 256-color palette.
    Palette(u8),
    /// Explicit true color.
    Rgb(Rgb),
}

/// Complete style attached to one terminal cell.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CellStyle {
    /// Foreground color source.
    pub foreground: ColorSpec,
    /// Background color source.
    pub background: ColorSpec,
    /// Underline color source.
    pub underline_color: ColorSpec,
    /// Bold attribute.
    pub bold: bool,
    /// Italic attribute.
    pub italic: bool,
    /// Faint attribute.
    pub faint: bool,
    /// Blink attribute.
    pub blink: bool,
    /// Inverse-video attribute.
    pub inverse: bool,
    /// Invisible attribute.
    pub invisible: bool,
    /// Strikethrough attribute.
    pub strikethrough: bool,
    /// Overline attribute.
    pub overline: bool,
    /// Upstream SGR underline value (`0` through `5`).
    pub underline: i32,
}

/// One visible cell in a semantic snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CellState {
    /// Zero-based column.
    pub column: u16,
    /// Full UTF-8 grapheme cluster; empty for a blank cell.
    pub grapheme: String,
    /// Visual style.
    pub style: CellStyle,
    /// Resolved explicit foreground, before terminal-default fallback.
    pub resolved_foreground: Option<Rgb>,
    /// Resolved explicit background, before terminal-default fallback.
    pub resolved_background: Option<Rgb>,
    /// Whether the cell is selected.
    pub selected: bool,
}

/// One viewport row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RowState {
    /// Zero-based viewport row.
    pub row: u16,
    /// Whether Ghostty marked this row dirty during the update.
    pub dirty: bool,
    /// All cells, including blanks.
    pub cells: Vec<CellState>,
}

/// Global render dirtiness reported by Ghostty.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DirtyState {
    /// No visual change.
    Clean,
    /// One or more rows changed.
    Partial,
    /// Full redraw required.
    Full,
}

/// Cursor visual style.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    /// Vertical bar.
    Bar,
    /// Filled block.
    Block,
    /// Underline.
    Underline,
    /// Hollow block.
    HollowBlock,
}

/// Cursor state at one semantic boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CursorState {
    /// Visibility mode.
    pub visible: bool,
    /// Blink mode.
    pub blinking: bool,
    /// Whether the cursor lies in the viewport.
    pub in_viewport: bool,
    /// Viewport column when available.
    pub column: Option<u16>,
    /// Viewport row when available.
    pub row: Option<u16>,
    /// Whether the cursor is on a wide-character tail.
    pub wide_tail: Option<bool>,
    /// Visual shape.
    pub style: CursorStyle,
}

/// Terminal modes relevant to presentation and input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModeState {
    /// DEC synchronized-output mode 2026.
    pub synchronized_output: bool,
    /// Any alternate-screen mode.
    pub alternate_screen: bool,
    /// DEC auto-wrap mode 7.
    pub wraparound: bool,
    /// DEC bracketed-paste mode 2004.
    pub bracketed_paste: bool,
    /// Complete stable inventory of known ANSI and DEC modes.
    pub values: Vec<TerminalMode>,
}

/// One ANSI or DEC mode value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalMode {
    /// Numeric mode identifier.
    pub number: u16,
    /// `true` for ANSI mode, `false` for DEC-private mode.
    pub ansi: bool,
    /// Current set/reset value.
    pub enabled: bool,
}

/// Active terminal colors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ColorState {
    /// Default/current background.
    pub background: Rgb,
    /// Default/current foreground.
    pub foreground: Rgb,
    /// Explicit cursor color.
    pub cursor: Option<Rgb>,
    /// Active 256-color palette.
    pub palette: Vec<Rgb>,
}

/// Terminal metadata controlled by OSC sequences.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalMetadata {
    /// Raw title bytes.
    pub title: ByteString,
    /// Raw working-directory bytes.
    pub working_directory: ByteString,
}

/// Authoritative semantic terminal state copied out of libghostty-vt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticState {
    /// Viewport columns.
    pub cols: u16,
    /// Viewport rows.
    pub rows: u16,
    /// Global render dirtiness.
    pub dirty: DirtyState,
    /// Rows marked dirty by Ghostty.
    pub dirty_rows: Vec<u16>,
    /// Cursor state.
    pub cursor: CursorState,
    /// Presentation/input modes.
    pub modes: ModeState,
    /// Active colors and palette.
    pub colors: ColorState,
    /// Current SGR style used by future printed cells.
    pub active_style: CellStyle,
    /// OSC-controlled terminal metadata.
    pub metadata: TerminalMetadata,
    /// Complete viewport grid.
    pub grid: Vec<RowState>,
}

/// Stable formatter outputs supported by libghostty-vt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    /// Plain UTF-8 text.
    Plain,
    /// VT stream that reconstructs styles and terminal state.
    Vt,
    /// Self-contained HTML fragment.
    Html,
}

/// Owned semantic engine and render snapshot state.
pub struct GhosttyVt {
    terminal: sys::GhosttyTerminal,
    render: sys::GhosttyRenderState,
    last_snapshot: Option<SemanticState>,
    responses: Box<ResponseState>,
}

#[derive(Default)]
struct ResponseState {
    pending: Vec<Vec<u8>>,
}

impl GhosttyVt {
    /// Create an engine with the given terminal geometry and scrollback limit.
    ///
    /// # Errors
    ///
    /// Returns an error when libghostty-vt cannot allocate either handle.
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self> {
        if cols == 0 || rows == 0 {
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
        });
    }

    /// Feed exact PTY output bytes through Ghostty's VT parser.
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { sys::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
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
        if let Some(state) = self.snapshot_if_changed()? {
            return Ok(state);
        }
        return self
            .last_snapshot
            .clone()
            .ok_or_else(|| return invalid_snapshot("unchanged terminal has no prior snapshot"));
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
        check("update render state", unsafe {
            sys::ghostty_render_state_update(self.render, self.terminal)
        })?;
        let cols = self.render_get(
            "get columns",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
        )?;
        let rows = self.render_get(
            "get rows",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
        )?;
        let dirty = self.dirty_state()?;
        let cursor = self.cursor_state()?;
        let modes = self.mode_state()?;
        let active_style = self.active_style()?;
        let metadata = self.terminal_metadata()?;
        let previous = self.last_snapshot.clone();
        if dirty == DirtyState::Clean
            && previous.as_ref().is_some_and(|state| {
                return state.cols == cols
                    && state.rows == rows
                    && state.cursor == cursor
                    && state.modes == modes
                    && state.active_style == active_style
                    && state.metadata == metadata;
            })
        {
            return Ok(None);
        }
        let colors = if dirty == DirtyState::Clean {
            previous
                .as_ref()
                .map(|state| return state.colors.clone())
                .unwrap_or(self.color_state()?)
        } else {
            self.color_state()?
        };
        let grid = self.grid(cols, rows, dirty, previous.as_ref())?;
        let dirty_rows = grid
            .iter()
            .filter_map(|row| return row.dirty.then_some(row.row))
            .collect();
        self.reset_render_dirty()?;
        let state = SemanticState {
            cols,
            rows,
            dirty,
            dirty_rows,
            cursor,
            modes,
            colors,
            active_style,
            metadata,
            grid,
        };
        if previous
            .as_ref()
            .is_some_and(|previous| return semantic_content_eq(previous, &state))
        {
            return Ok(None);
        }
        self.last_snapshot = Some(state.clone());
        return Ok(Some(state));
    }

    /// Format the active screen using Ghostty's own formatter.
    ///
    /// # Errors
    ///
    /// Returns an error when formatter creation or output fails.
    pub fn format(&self, format: Format) -> Result<Vec<u8>> {
        let emit = match format {
            Format::Plain => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Format::Vt => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
            Format::Html => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_HTML,
        };
        let screen = sys::GhosttyFormatterScreenExtra {
            size: size_of::<sys::GhosttyFormatterScreenExtra>(),
            cursor: true,
            style: true,
            hyperlink: true,
            protection: true,
            kitty_keyboard: true,
            charsets: true,
        };
        let extra = sys::GhosttyFormatterTerminalExtra {
            size: size_of::<sys::GhosttyFormatterTerminalExtra>(),
            palette: true,
            modes: true,
            scrolling_region: true,
            tabstops: true,
            pwd: true,
            keyboard: true,
            screen,
        };
        let options = sys::GhosttyFormatterTerminalOptions {
            size: size_of::<sys::GhosttyFormatterTerminalOptions>(),
            emit,
            unwrap: false,
            trim: true,
            extra,
            selection: ptr::null(),
        };
        let formatter = Formatter::new(self.terminal, options)?;
        return formatter.output();
    }

    fn render_get<T: Default>(&self, operation: &'static str, key: u32) -> Result<T> {
        let mut value = T::default();
        check(operation, unsafe {
            sys::ghostty_render_state_get(self.render, key, (&raw mut value).cast::<c_void>())
        })?;
        return Ok(value);
    }

    fn active_style(&self) -> Result<CellStyle> {
        let mut style = sys::GhosttyStyle {
            size: size_of::<sys::GhosttyStyle>(),
            ..Default::default()
        };
        check("get active cursor style", unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_STYLE,
                (&raw mut style).cast::<c_void>(),
            )
        })?;
        return decode_style(style);
    }

    fn terminal_metadata(&self) -> Result<TerminalMetadata> {
        return Ok(TerminalMetadata {
            title: ByteString::new(self.terminal_string(
                "get terminal title",
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE,
            )?),
            working_directory: ByteString::new(self.terminal_string(
                "get terminal working directory",
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
            )?),
        });
    }

    fn terminal_string(&self, operation: &'static str, key: u32) -> Result<Vec<u8>> {
        let mut value = sys::GhosttyString::default();
        check(operation, unsafe {
            sys::ghostty_terminal_get(self.terminal, key, (&raw mut value).cast::<c_void>())
        })?;
        if value.len == 0 {
            return Ok(Vec::new());
        }
        if value.ptr.is_null() {
            return Err(invalid_snapshot(
                "terminal string has a non-zero length and null pointer",
            ));
        }
        return Ok(unsafe { slice::from_raw_parts(value.ptr, value.len) }.to_vec());
    }

    fn mode(&self, value: u16, ansi: bool) -> Result<bool> {
        let mut enabled = false;
        let packed = (value & 0x7fff) | (u16::from(ansi) << 15);
        check("get terminal mode", unsafe {
            sys::ghostty_terminal_mode_get(self.terminal, packed, &raw mut enabled)
        })?;
        return Ok(enabled);
    }

    fn mode_state(&self) -> Result<ModeState> {
        const ANSI_MODES: &[u16] = &[2, 4, 12, 20];
        const DEC_MODES: &[u16] = &[
            1, 3, 4, 5, 6, 7, 8, 9, 12, 25, 40, 45, 47, 66, 67, 69, 1000, 1002, 1003, 1004, 1005,
            1006, 1007, 1015, 1016, 1035, 1036, 1039, 1045, 1047, 1048, 1049, 2004, 2026, 2027,
            2031, 2048,
        ];
        let mut values = Vec::with_capacity(ANSI_MODES.len() + DEC_MODES.len());
        for &number in ANSI_MODES {
            values.push(TerminalMode {
                number,
                ansi: true,
                enabled: self.mode(number, true)?,
            });
        }
        for &number in DEC_MODES {
            values.push(TerminalMode {
                number,
                ansi: false,
                enabled: self.mode(number, false)?,
            });
        }
        let dec = |number| {
            return values
                .iter()
                .find(|mode| return !mode.ansi && mode.number == number)
                .is_some_and(|mode| return mode.enabled);
        };
        return Ok(ModeState {
            synchronized_output: dec(2026),
            alternate_screen: dec(47) || dec(1047) || dec(1049),
            wraparound: dec(7),
            bracketed_paste: dec(2004),
            values,
        });
    }

    fn dirty_state(&self) -> Result<DirtyState> {
        let dirty: u32 = self.render_get(
            "get dirty state",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY,
        )?;
        match dirty {
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE => {
                return Ok(DirtyState::Clean);
            }
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                return Ok(DirtyState::Partial);
            }
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => {
                return Ok(DirtyState::Full);
            }
            result => {
                return Err(Error::GhosttyVt {
                    operation: "decode dirty state",
                    result: i32::try_from(result).unwrap_or(i32::MAX),
                });
            }
        }
    }

    fn cursor_state(&self) -> Result<CursorState> {
        let visible = self.render_get(
            "get cursor visibility",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
        )?;
        let blinking = self.render_get(
            "get cursor blinking",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING,
        )?;
        let in_viewport: bool = self.render_get(
            "get cursor viewport presence",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
        )?;
        let (column, row, wide_tail) = if in_viewport {
            (
                Some(self.render_get(
                    "get cursor column",
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
                )?),
                Some(self.render_get(
                    "get cursor row",
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
                )?),
                Some(self.render_get(
                    "get cursor wide tail",
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
                )?),
            )
        } else {
            (None, None, None)
        };
        let raw_style: u32 = self.render_get(
            "get cursor style",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
        )?;
        let style = decode_cursor_style(raw_style)?;
        return Ok(CursorState {
            visible,
            blinking,
            in_viewport,
            column,
            row,
            wide_tail,
            style,
        });
    }

    fn color_state(&self) -> Result<ColorState> {
        let mut colors = sys::GhosttyRenderStateColors {
            size: size_of::<sys::GhosttyRenderStateColors>(),
            ..Default::default()
        };
        check("get render colors", unsafe {
            sys::ghostty_render_state_colors_get(self.render, &raw mut colors)
        })?;
        return Ok(ColorState {
            background: colors.background.into(),
            foreground: colors.foreground.into(),
            cursor: colors.cursor_has_value.then(|| return colors.cursor.into()),
            palette: colors.palette.into_iter().map(Into::into).collect(),
        });
    }

    fn grid(
        &self,
        cols: u16,
        rows: u16,
        dirty: DirtyState,
        previous: Option<&SemanticState>,
    ) -> Result<Vec<RowState>> {
        let reusable = previous.filter(|state| return state.cols == cols && state.rows == rows);
        if dirty == DirtyState::Clean
            && let Some(previous) = reusable
        {
            let mut grid = previous.grid.clone();
            for row in &mut grid {
                row.dirty = false;
            }
            return Ok(grid);
        }
        let read_all = dirty == DirtyState::Full || reusable.is_none();
        let mut handles = RenderIterators::new()?;
        check("populate row iterator", unsafe {
            sys::ghostty_render_state_get(
                self.render,
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut handles.row).cast::<c_void>(),
            )
        })?;
        let mut grid = Vec::with_capacity(usize::from(rows));
        while unsafe { sys::ghostty_render_state_row_iterator_next(handles.row) } {
            let row = u16::try_from(grid.len()).unwrap_or(u16::MAX);
            let row_dirty = row_get(
                handles.row,
                "get row dirty state",
                sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
            )?;
            let cells = if read_all || row_dirty {
                check("populate row cells", unsafe {
                    sys::ghostty_render_state_row_get(
                        handles.row,
                        sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                        (&raw mut handles.cells).cast::<c_void>(),
                    )
                })?;
                let mut cells = Vec::with_capacity(usize::from(cols));
                for column in 0..cols {
                    if !unsafe { sys::ghostty_render_state_row_cells_next(handles.cells) } {
                        return Err(Error::GhosttyVt {
                            operation: "iterate row cells",
                            result: sys::GhosttyResult_GHOSTTY_NO_VALUE,
                        });
                    }
                    cells.push(read_cell(handles.cells, column)?);
                }
                cells
            } else {
                reusable
                    .and_then(|state| return state.grid.get(usize::from(row)))
                    .map(|state| return state.cells.clone())
                    .ok_or_else(|| {
                        return invalid_snapshot("clean row is absent from previous snapshot");
                    })?
            };
            let clean = false;
            check("reset row dirty state", unsafe {
                sys::ghostty_render_state_row_set(
                    handles.row,
                    sys::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                    (&raw const clean).cast::<c_void>(),
                )
            })?;
            grid.push(RowState {
                row,
                dirty: row_dirty,
                cells,
            });
        }
        if grid.len() != usize::from(rows) {
            return Err(invalid_snapshot(
                "render row count differs from terminal rows",
            ));
        }
        return Ok(grid);
    }

    fn reset_render_dirty(&self) -> Result<()> {
        let clean = sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        return check("reset render dirty state", unsafe {
            sys::ghostty_render_state_set(
                self.render,
                sys::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&raw const clean).cast::<c_void>(),
            )
        });
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

struct RenderIterators {
    row: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
}

impl RenderIterators {
    fn new() -> Result<Self> {
        let mut row = ptr::null_mut();
        check("create row iterator", unsafe {
            sys::ghostty_render_state_row_iterator_new(ptr::null(), &raw mut row)
        })?;
        let mut cells = ptr::null_mut();
        if let Err(error) = check("create row cells", unsafe {
            sys::ghostty_render_state_row_cells_new(ptr::null(), &raw mut cells)
        }) {
            unsafe { sys::ghostty_render_state_row_iterator_free(row) };
            return Err(error);
        }
        return Ok(Self { row, cells });
    }
}

impl Drop for RenderIterators {
    fn drop(&mut self) {
        unsafe {
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.row);
        }
    }
}

struct Formatter(sys::GhosttyFormatter);

impl Formatter {
    fn new(
        terminal: sys::GhosttyTerminal,
        options: sys::GhosttyFormatterTerminalOptions,
    ) -> Result<Self> {
        let mut formatter = ptr::null_mut();
        check("create terminal formatter", unsafe {
            sys::ghostty_formatter_terminal_new(ptr::null(), &raw mut formatter, terminal, options)
        })?;
        return Ok(Self(formatter));
    }

    fn output(&self) -> Result<Vec<u8>> {
        let mut length = 0_usize;
        let size_result = unsafe {
            sys::ghostty_formatter_format_buf(self.0, ptr::null_mut(), 0, &raw mut length)
        };
        if size_result == sys::GhosttyResult_GHOSTTY_SUCCESS && length == 0 {
            return Ok(Vec::new());
        }
        if size_result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            return Err(Error::GhosttyVt {
                operation: "size formatter output",
                result: size_result,
            });
        }
        let mut output = vec![0_u8; length];
        check("format terminal", unsafe {
            sys::ghostty_formatter_format_buf(
                self.0,
                output.as_mut_ptr(),
                output.len(),
                &raw mut length,
            )
        })?;
        output.truncate(length);
        return Ok(output);
    }
}

impl Drop for Formatter {
    fn drop(&mut self) {
        unsafe { sys::ghostty_formatter_free(self.0) };
    }
}

fn row_get<T: Default>(
    row: sys::GhosttyRenderStateRowIterator,
    operation: &'static str,
    key: u32,
) -> Result<T> {
    let mut value = T::default();
    check(operation, unsafe {
        sys::ghostty_render_state_row_get(row, key, (&raw mut value).cast::<c_void>())
    })?;
    return Ok(value);
}

fn cell_get<T: Default>(
    cells: sys::GhosttyRenderStateRowCells,
    operation: &'static str,
    key: u32,
) -> Result<T> {
    let mut value = T::default();
    check(operation, unsafe {
        sys::ghostty_render_state_row_cells_get(cells, key, (&raw mut value).cast::<c_void>())
    })?;
    return Ok(value);
}

fn read_cell(cells: sys::GhosttyRenderStateRowCells, column: u16) -> Result<CellState> {
    let mut style = sys::GhosttyStyle {
        size: size_of::<sys::GhosttyStyle>(),
        ..Default::default()
    };
    check("get cell style", unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            (&raw mut style).cast::<c_void>(),
        )
    })?;
    return Ok(CellState {
        column,
        grapheme: read_grapheme(cells)?,
        style: decode_style(style)?,
        resolved_foreground: optional_cell_color(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
        )?,
        resolved_background: optional_cell_color(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        )?,
        selected: cell_get(
            cells,
            "get cell selection",
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED,
        )?,
    });
}

fn read_grapheme(cells: sys::GhosttyRenderStateRowCells) -> Result<String> {
    let mut buffer = sys::GhosttyBuffer::default();
    let first = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            (&raw mut buffer).cast::<c_void>(),
        )
    };
    if first == sys::GhosttyResult_GHOSTTY_SUCCESS && buffer.len == 0 {
        return Ok(String::new());
    }
    if first != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        return Err(Error::GhosttyVt {
            operation: "size cell grapheme",
            result: first,
        });
    }
    let mut bytes = vec![0_u8; buffer.len];
    buffer.ptr = bytes.as_mut_ptr();
    buffer.cap = bytes.len();
    check("get cell grapheme", unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            (&raw mut buffer).cast::<c_void>(),
        )
    })?;
    bytes.truncate(buffer.len);
    return String::from_utf8(bytes).map_err(|_| {
        return Error::GhosttyVt {
            operation: "decode cell grapheme",
            result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
        };
    });
}

fn optional_cell_color(cells: sys::GhosttyRenderStateRowCells, key: u32) -> Result<Option<Rgb>> {
    let mut color = sys::GhosttyColorRgb::default();
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(cells, key, (&raw mut color).cast::<c_void>())
    };
    match result {
        sys::GhosttyResult_GHOSTTY_SUCCESS => return Ok(Some(color.into())),
        sys::GhosttyResult_GHOSTTY_INVALID_VALUE => return Ok(None),
        result => {
            return Err(Error::GhosttyVt {
                operation: "get cell color",
                result,
            });
        }
    }
}

fn decode_style(style: sys::GhosttyStyle) -> Result<CellStyle> {
    return Ok(CellStyle {
        foreground: decode_color(style.fg_color)?,
        background: decode_color(style.bg_color)?,
        underline_color: decode_color(style.underline_color)?,
        bold: style.bold,
        italic: style.italic,
        faint: style.faint,
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        underline: style.underline,
    });
}

fn decode_color(color: sys::GhosttyStyleColor) -> Result<ColorSpec> {
    match color.tag {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => return Ok(ColorSpec::None),
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            return Ok(ColorSpec::Palette(unsafe { color.value.palette }));
        }
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            return Ok(ColorSpec::Rgb(unsafe { color.value.rgb }.into()));
        }
        result => {
            return Err(Error::GhosttyVt {
                operation: "decode style color",
                result: i32::try_from(result).unwrap_or(i32::MAX),
            });
        }
    }
}

fn decode_cursor_style(style: u32) -> Result<CursorStyle> {
    match style {
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
            return Ok(CursorStyle::Bar)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
            return Ok(CursorStyle::Block)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
            return Ok(CursorStyle::Underline)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
            return Ok(CursorStyle::HollowBlock)
        }
        result => return Err(Error::GhosttyVt {
            operation: "decode cursor style",
            result: i32::try_from(result).unwrap_or(i32::MAX),
        }),
    }
}

fn check(operation: &'static str, result: sys::GhosttyResult) -> Result<()> {
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS {
        return Ok(());
    } else {
        return Err(Error::GhosttyVt { operation, result });
    }
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

fn invalid_snapshot(operation: &'static str) -> Error {
    return Error::GhosttyVt {
        operation,
        result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
    };
}

fn semantic_content_eq(left: &SemanticState, right: &SemanticState) -> bool {
    return left.cols == right.cols
        && left.rows == right.rows
        && left.cursor == right.cursor
        && left.modes == right.modes
        && left.colors == right.colors
        && left.active_style == right.active_style
        && left.metadata == right.metadata
        && left.grid.len() == right.grid.len()
        && left
            .grid
            .iter()
            .zip(&right.grid)
            .all(|(left, right)| return left.row == right.row && left.cells == right.cells);
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(color: sys::GhosttyColorRgb) -> Self {
        return Self {
            r: color.r,
            g: color.g,
            b: color.b,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_unicode_truecolor_cursor_modes_and_alternate_screen() {
        let mut terminal = GhosttyVt::new(12, 4, 100).expect("create terminal");
        terminal.write(b"plain \x1b[38;2;1;2;3m");
        terminal.write("界e\u{301}".as_bytes());
        terminal.write(b"\x1b[0m\x1b[2;3H\x1b[?2004h");
        let primary = terminal.snapshot().expect("primary snapshot");
        assert_eq!(primary.cols, 12);
        assert_eq!(primary.rows, 4);
        assert!(primary.modes.bracketed_paste);
        assert_eq!(primary.cursor.column, Some(2));
        assert_eq!(primary.cursor.row, Some(1));
        assert!(
            primary.grid[0]
                .cells
                .iter()
                .any(|cell| return cell.grapheme == "界")
        );
        assert!(
            primary.grid[0]
                .cells
                .iter()
                .any(|cell| return cell.grapheme == "e\u{301}")
        );
        assert!(
            primary.grid[0]
                .cells
                .iter()
                .any(|cell| { return cell.resolved_foreground == Some(Rgb { r: 1, g: 2, b: 3 }) })
        );

        terminal.write(b"\x1b[?1049hALT");
        let alternate = terminal.snapshot().expect("alternate snapshot");
        assert!(alternate.modes.alternate_screen);
        assert!(
            alternate
                .grid
                .iter()
                .flat_map(|row| return &row.cells)
                .any(|cell| return cell.grapheme == "A")
        );
        terminal.write(b"\x1b[?1049l");
        let restored = terminal.snapshot().expect("restored snapshot");
        assert!(!restored.modes.alternate_screen);
        assert_eq!(restored.grid[0].cells[0].grapheme, "p");
        assert!(
            String::from_utf8(terminal.format(Format::Plain).expect("plain format"))
                .expect("plain UTF-8")
                .contains("plain")
        );
        assert!(
            String::from_utf8(terminal.format(Format::Html).expect("HTML format"))
                .expect("HTML UTF-8")
                .contains("plain")
        );
        assert!(
            terminal
                .format(Format::Vt)
                .expect("VT format")
                .contains(&0x1b)
        );
    }

    #[test]
    fn tracks_synchronized_output_and_resize() {
        let mut terminal = GhosttyVt::new(8, 2, 0).expect("create terminal");
        terminal.write(b"\x1b[?2026hhidden");
        assert!(
            terminal
                .snapshot()
                .expect("sync snapshot")
                .modes
                .synchronized_output
        );
        terminal.write(b"\x1b[?2026l");
        assert!(
            !terminal
                .snapshot()
                .expect("presented snapshot")
                .modes
                .synchronized_output
        );
        terminal.resize(10, 3, 9, 18).expect("resize");
        let resized = terminal.snapshot().expect("resized snapshot");
        assert_eq!((resized.cols, resized.rows), (10, 3));
    }

    #[test]
    fn emits_terminal_query_responses() {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        terminal.write(b"\x1b[3;5H\x1b[6n");
        assert_eq!(terminal.take_responses(), vec![b"\x1b[3;5R".to_vec()]);
        assert!(terminal.take_responses().is_empty());
    }
}
