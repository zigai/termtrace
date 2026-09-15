//! Semantic snapshot extraction and Ghostty formatter output.

use super::{
    sys,
    terminal::check,
    types::{
        CellState, CellStyle, ColorSpec, ColorState, CursorState, CursorStyle, DirtyState, Format,
        ModeState, Rgb, RowState, SemanticState, TerminalMetadata, TerminalMode, WideKind,
    },
};
use crate::{Error, Result};
use compact_str::CompactString;
use std::{ffi::c_void, mem::size_of, ptr, slice, str};
use termtrace_model::value::ByteString;

pub(super) struct Snapshot<'engine> {
    terminal: sys::GhosttyTerminal,
    render: sys::GhosttyRenderState,
    last_snapshot: &'engine mut Option<SemanticState>,
}

impl<'engine> Snapshot<'engine> {
    pub(super) fn new(
        terminal: sys::GhosttyTerminal,
        render: sys::GhosttyRenderState,
        last_snapshot: &'engine mut Option<SemanticState>,
    ) -> Self {
        return Self {
            terminal,
            render,
            last_snapshot,
        };
    }

    pub(super) fn snapshot(&mut self) -> Result<SemanticState> {
        if let Some(state) = self.snapshot_if_changed()? {
            return Ok(state);
        }

        let mut state = self.last_snapshot.as_ref().cloned().ok_or_else(|| {
            return invalid_snapshot("unchanged terminal has no prior snapshot");
        })?;
        state.dirty = DirtyState::Clean;

        state.dirty_rows.clear();

        for row in &mut state.grid {
            row.dirty = false;
        }

        return Ok(state);
    }

    pub(super) fn snapshot_if_changed(&mut self) -> Result<Option<SemanticState>> {
        check("update render state", unsafe {
            sys::ghostty_render_state_update(self.render, self.terminal)
        })?;

        let dirty = self.dirty_state()?;
        let cols = self.render_get(
            "get columns",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
        )?;
        let rows = self.render_get(
            "get rows",
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
        )?;
        let cursor = self.cursor_state()?;
        let active_style = self.active_style()?;
        let metadata = self.terminal_metadata()?;

        if dirty == DirtyState::Clean {
            let unchanged = match self.last_snapshot.as_ref() {
                Some(state) if state.cols == cols && state.rows == rows => {
                    state.cursor == cursor
                        && state.active_style == active_style
                        && state.metadata == metadata
                        && self.modes_equal(&state.modes)?
                }
                _ => false,
            };

            if unchanged {
                return Ok(None);
            }

            let modes_outcome = match self.last_snapshot.as_ref() {
                Some(state) if state.cols == cols && state.rows == rows => {
                    if self.modes_equal(&state.modes)? {
                        Some(None)
                    } else {
                        Some(Some(self.mode_state()?))
                    }
                }
                _ => None,
            };
            if let Some(new_modes) = modes_outcome {
                self.reset_render_dirty()?;

                let Some(state) = self.last_snapshot.as_mut() else {
                    return Ok(None);
                };
                state.cursor = cursor;

                if let Some(modes) = new_modes {
                    state.modes = modes;
                }

                state.active_style = active_style;
                state.metadata = metadata;
                state.dirty = DirtyState::Clean;
                state.dirty_rows.clear();

                for row in &mut state.grid {
                    row.dirty = false;
                }

                return Ok(Some(state.clone()));
            }
        }

        let previous = self.last_snapshot.as_ref();

        let modes = match previous {
            Some(state) if self.modes_equal(&state.modes)? => state.modes.clone(),
            _ => self.mode_state()?,
        };

        let colors = if dirty == DirtyState::Clean {
            match previous {
                Some(state) => state.colors.clone(),
                None => self.color_state()?,
            }
        } else {
            self.color_state()?
        };

        let grid = self.grid(cols, rows, dirty, previous)?;
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

        if previous.is_some_and(|previous| return semantic_content_eq(previous, &state)) {
            return Ok(None);
        }

        *self.last_snapshot = Some(state.clone());

        return Ok(Some(state));
    }
}

pub(super) fn format(terminal: sys::GhosttyTerminal, format: Format) -> Result<Vec<u8>> {
    return format_with_unwrap(terminal, format, false);
}

pub(super) fn format_replay_seed(terminal: sys::GhosttyTerminal) -> Result<Option<Vec<u8>>> {
    let mut pending_wrap = false;
    // SAFETY: `terminal` is the live handle owned by `GhosttyVt`, and the selected key writes one
    // initialized `bool` through the non-null, correctly aligned output pointer.
    check("get cursor pending-wrap state", unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP,
            (&raw mut pending_wrap).cast::<c_void>(),
        )
    })?;

    if pending_wrap {
        return Ok(None);
    }
    return format_with_unwrap(terminal, Format::Vt, true).map(Some);
}

fn format_with_unwrap(
    terminal: sys::GhosttyTerminal,
    format: Format,
    unwrap: bool,
) -> Result<Vec<u8>> {
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
        unwrap,
        trim: true,
        extra,
        selection: ptr::null(),
    };
    let formatter = Formatter::new(terminal, options)?;

    return formatter.output();
}

impl Snapshot<'_> {
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

    fn modes_equal(&self, previous: &ModeState) -> Result<bool> {
        if previous.synchronized_output != self.mode(2026, false)? {
            return Ok(false);
        }

        if previous.bracketed_paste != self.mode(2004, false)? {
            return Ok(false);
        }

        if previous.wraparound != self.mode(7, false)? {
            return Ok(false);
        }

        for mode in &previous.values {
            if matches!(mode.number, 2026 | 2004 | 7) && !mode.ansi {
                continue;
            }

            if self.mode(mode.number, mode.ansi)? != mode.enabled {
                return Ok(false);
            }
        }

        return Ok(true);
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

        let mut synchronized_output = false;
        let mut alternate_screen = false;
        let mut wraparound = false;
        let mut bracketed_paste = false;

        for &number in DEC_MODES {
            let enabled = self.mode(number, false)?;
            match number {
                2026 => synchronized_output = enabled,
                47 | 1047 | 1049 => alternate_screen |= enabled,
                7 => wraparound = enabled,
                2004 => bracketed_paste = enabled,
                _ => {}
            }

            values.push(TerminalMode {
                number,
                ansi: false,
                enabled,
            });
        }

        return Ok(ModeState {
            synchronized_output,
            alternate_screen,
            wraparound,
            bracketed_paste,
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
            let mut row_state = if read_all || row_dirty {
                let raw_row: sys::GhosttyRow = row_get(
                    handles.row,
                    "get raw row",
                    sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                )?;

                let soft_wrapped = raw_row_get(
                    raw_row,
                    "get row wrap",
                    sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
                )?;

                let wrap_continuation = raw_row_get(
                    raw_row,
                    "get row wrap continuation",
                    sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                )?;

                check("populate row cells", unsafe {
                    sys::ghostty_render_state_row_get(
                        handles.row,
                        sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                        (&raw mut handles.cells).cast::<c_void>(),
                    )
                })?;

                let mut selection = sys::GhosttyRenderStateRowSelection {
                    size: size_of::<sys::GhosttyRenderStateRowSelection>(),
                    ..Default::default()
                };
                let has_selection = unsafe {
                    sys::ghostty_render_state_row_get(
                        handles.row,
                        sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                        (&raw mut selection).cast::<c_void>(),
                    ) == sys::GhosttyResult_GHOSTTY_SUCCESS
                };

                let mut cells = Vec::with_capacity(usize::from(cols));
                for column in 0..cols {
                    if !unsafe { sys::ghostty_render_state_row_cells_next(handles.cells) } {
                        return Err(Error::GhosttyVt {
                            operation: "iterate row cells",
                            result: sys::GhosttyResult_GHOSTTY_NO_VALUE,
                        });
                    }

                    let selected =
                        has_selection && column >= selection.start_x && column <= selection.end_x;
                    cells.push(read_cell(handles.cells, column, selected)?);
                }

                RowState {
                    row,
                    soft_wrapped,
                    wrap_continuation,
                    dirty: row_dirty,
                    cells,
                }
            } else {
                reusable
                    .and_then(|state| return state.grid.get(usize::from(row)))
                    .filter(|state| return state.row == row)
                    .cloned()
                    .ok_or_else(|| {
                        return invalid_snapshot("clean row is absent from previous snapshot");
                    })?
            };
            row_state.dirty = row_dirty;
            let clean = false;
            check("reset row dirty state", unsafe {
                sys::ghostty_render_state_row_set(
                    handles.row,
                    sys::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                    (&raw const clean).cast::<c_void>(),
                )
            })?;

            grid.push(row_state);
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

fn raw_row_get<T: Default>(row: sys::GhosttyRow, operation: &'static str, key: u32) -> Result<T> {
    let mut value = T::default();
    check(operation, unsafe {
        sys::ghostty_row_get(row, key, (&raw mut value).cast::<c_void>())
    })?;

    return Ok(value);
}

fn raw_cell_get<T: Default>(
    cell: sys::GhosttyCell,
    operation: &'static str,
    key: u32,
) -> Result<T> {
    let mut value = T::default();
    check(operation, unsafe {
        sys::ghostty_cell_get(cell, key, (&raw mut value).cast::<c_void>())
    })?;

    return Ok(value);
}

fn read_cell(
    cells: sys::GhosttyRenderStateRowCells,
    column: u16,
    selected: bool,
) -> Result<CellState> {
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
        wide_kind: read_wide_kind(cells)?,
        style: decode_style(style)?,
        resolved_foreground: optional_cell_color(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
        )?,
        resolved_background: optional_cell_color(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        )?,
        selected,
    });
}

fn read_wide_kind(cells: sys::GhosttyRenderStateRowCells) -> Result<WideKind> {
    let cell: sys::GhosttyCell = cell_get(
        cells,
        "get raw cell",
        sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
    )?;

    let wide: sys::GhosttyCellWide = raw_cell_get(
        cell,
        "get cell wide kind",
        sys::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
    )?;

    return decode_wide_kind(wide);
}

fn decode_wide_kind(wide: sys::GhosttyCellWide) -> Result<WideKind> {
    match wide {
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => return Ok(WideKind::Narrow),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => return Ok(WideKind::Wide),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => return Ok(WideKind::SpacerTail),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => return Ok(WideKind::SpacerHead),
        result => {
            return Err(Error::GhosttyVt {
                operation: "decode cell wide kind",
                result: i32::try_from(result).unwrap_or(i32::MAX),
            });
        }
    }
}

fn read_grapheme(cells: sys::GhosttyRenderStateRowCells) -> Result<CompactString> {
    let mut stack_buffer = [0_u8; 24];
    let mut buffer = sys::GhosttyBuffer {
        ptr: stack_buffer.as_mut_ptr(),
        cap: stack_buffer.len(),
        len: 0,
    };
    let first = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            (&raw mut buffer).cast::<c_void>(),
        )
    };

    if first == sys::GhosttyResult_GHOSTTY_SUCCESS {
        if buffer.len == 0 {
            return Ok(CompactString::new(""));
        }

        if let Some(bytes) = stack_buffer.get(..buffer.len) {
            let grapheme = str::from_utf8(bytes).map_err(|_| {
                return Error::GhosttyVt {
                    operation: "decode cell grapheme",
                    result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
                };
            })?;

            return Ok(CompactString::new(grapheme));
        }
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
    let text = str::from_utf8(&bytes).map_err(|_| {
        return Error::GhosttyVt {
            operation: "decode cell grapheme",
            result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
        };
    })?;

    return Ok(CompactString::new(text));
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
            return Ok(CursorStyle::Bar);
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
            return Ok(CursorStyle::Block);
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
            return Ok(CursorStyle::Underline);
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
            return Ok(CursorStyle::HollowBlock);
        }
        result => {
            return Err(Error::GhosttyVt {
                operation: "decode cursor style",
                result: i32::try_from(result).unwrap_or(i32::MAX),
            });
        }
    }
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
        && left.grid.iter().zip(&right.grid).all(|(left, right)| {
            return left.row == right.row
                && left.soft_wrapped == right.soft_wrapped
                && left.wrap_continuation == right.wrap_continuation
                && left.cells == right.cells;
        });
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
    use crate::ghostty_vt::terminal::GhosttyVt;

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
        assert!(primary.grid[0]
            .cells
            .iter()
            .any(|cell| return cell.grapheme == "界"));
        assert!(primary.grid[0]
            .cells
            .iter()
            .any(|cell| return cell.grapheme == "e\u{301}"));
        assert!(primary.grid[0].cells.iter().any(|cell| {
            return cell.resolved_foreground == Some(Rgb { r: 1, g: 2, b: 3 });
        }));

        terminal.write(b"\x1b[?1049hALT");
        let alternate = terminal.snapshot().expect("alternate snapshot");
        assert!(alternate.modes.alternate_screen);
        assert!(alternate
            .grid
            .iter()
            .flat_map(|row| return &row.cells)
            .any(|cell| return cell.grapheme == "A"));

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
        assert!(terminal
            .format(Format::Vt)
            .expect("VT format")
            .contains(&0x1b));
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
    fn preserves_native_wrap_and_wide_facts_across_clean_reuse() {
        let mut terminal = GhosttyVt::new(4, 3, 0).expect("create terminal");
        terminal.write("abc界".as_bytes());
        let first = terminal.snapshot().expect("first snapshot");
        assert!(first.grid.iter().any(|row| return row.soft_wrapped));
        assert!(first.grid.iter().any(|row| return row.wrap_continuation));
        assert!(first
            .grid
            .iter()
            .flat_map(|row| return &row.cells)
            .any(|cell| {
                return cell.wide_kind == WideKind::Wide;
            }));
        assert!(first
            .grid
            .iter()
            .flat_map(|row| return &row.cells)
            .any(|cell| {
                return cell.wide_kind == WideKind::SpacerTail;
            }));
        assert!(first
            .grid
            .iter()
            .flat_map(|row| return &row.cells)
            .any(|cell| {
                return cell.wide_kind == WideKind::SpacerHead;
            }));

        let reused = terminal.snapshot().expect("clean reused snapshot");
        assert!(reused.grid.iter().all(|row| return !row.dirty));
        assert!(first.grid.iter().zip(&reused.grid).all(|(left, right)| {
            return left.soft_wrapped == right.soft_wrapped
                && left.wrap_continuation == right.wrap_continuation
                && left.cells == right.cells;
        }));
    }

    #[test]
    fn rejects_unknown_native_wide_kind() {
        assert!(matches!(
            decode_wide_kind(sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_MAX_VALUE),
            Err(Error::GhosttyVt {
                operation: "decode cell wide kind",
                ..
            })
        ));
    }

    #[test]
    fn captures_graphemes_across_stack_and_heap_thresholds_and_tracks_modes() {
        let mut terminal = GhosttyVt::new(10, 2, 0).expect("create terminal");
        let multi_combining = "e\u{300}\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}";
        terminal.write(format!("a界{multi_combining}").as_bytes());
        let snapshot = terminal.snapshot().expect("snapshot");
        assert!(snapshot.grid[0]
            .cells
            .iter()
            .any(|cell| return cell.grapheme == "a"));
        assert!(snapshot.grid[0]
            .cells
            .iter()
            .any(|cell| return cell.grapheme == "界"));
        assert!(snapshot.grid[0]
            .cells
            .iter()
            .any(|cell| return cell.grapheme == multi_combining));

        let unchanged = terminal.snapshot_if_changed().expect("snapshot_if_changed");
        assert!(unchanged.is_none());

        terminal.write(b"\x1b[?2004h");
        let changed = terminal
            .snapshot_if_changed()
            .expect("bracketed paste enabled");
        assert!(changed.expect("changed state").modes.bracketed_paste);

        let unchanged2 = terminal.snapshot_if_changed().expect("snapshot_if_changed");
        assert!(unchanged2.is_none());

        terminal.write(b"\x1b[?2004l");
        let changed2 = terminal
            .snapshot_if_changed()
            .expect("bracketed paste disabled");
        assert!(!changed2.expect("changed state 2").modes.bracketed_paste);
    }
}
