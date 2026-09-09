//! Stable Rust value types exposed by the Ghostty VT adapter.

use crate::value::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest supported terminal width at any trusted or imported boundary.
pub const MAX_TERMINAL_COLUMNS: u16 = 1_000;
/// Largest supported terminal height at any trusted or imported boundary.
pub const MAX_TERMINAL_ROWS: u16 = 1_000;
/// Largest supported viewport allocation, independent of aspect ratio.
pub const MAX_TERMINAL_CELLS: u32 = 250_000;

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

/// Layout-independent key accepted by the terminal key encoder adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamedKey {
    /// Enter/Return terminal key (distinct from raw line feed).
    Enter,
    /// Escape key.
    Escape,
    /// Horizontal tab key.
    Tab,
    /// Backspace key.
    Backspace,
    /// Forward delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page-up key.
    PageUp,
    /// Page-down key.
    PageDown,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Function key numbered 1 through 24.
    Function(u8),
    /// ASCII letter used for Ctrl+A through Ctrl+Z.
    Letter(char),
}

/// Typed modifier state for one semantic key event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Keyboard modifiers are independently combinable input protocol bits"
)]
pub struct KeyModifiers {
    /// Shift modifier.
    pub shift: bool,
    /// Control modifier.
    pub control: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Super modifier.
    pub super_key: bool,
}

/// Result of encoding a semantic key against current terminal modes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EncodedKey {
    /// Exact bytes to write to the PTY.
    pub data: ByteString,
    /// Stable encoder identity.
    pub encoder: String,
}

/// Terminal-window focus transition reported to an opted-in application.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FocusEvent {
    /// The terminal window gained focus.
    In,
    /// The terminal window lost focus.
    Out,
}

/// Stable mouse button identity accepted by the pinned encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
    /// Vertical wheel up.
    WheelUp,
    /// Vertical wheel down.
    WheelDown,
}

/// Stable mouse action accepted by the pinned encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    /// Button press or wheel tick.
    Press,
    /// Button release.
    Release,
    /// Pointer motion while a button remains logically pressed.
    Motion,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "SGR attributes are independently combinable terminal style bits"
)]
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

/// Authoritative width role reported by Ghostty for one terminal cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WideKind {
    /// A regular one-column cell.
    Narrow,
    /// The leading cell of a two-column grapheme.
    Wide,
    /// The non-rendered cell following a wide grapheme.
    SpacerTail,
    /// A wrap-edge spacer preceding a wide grapheme on the next row.
    SpacerHead,
}

/// One visible cell in a semantic snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CellState {
    /// Zero-based column.
    pub column: u16,
    /// Full UTF-8 grapheme cluster; empty for a blank cell.
    pub grapheme: String,
    /// Native width role; never inferred from the grapheme.
    pub wide_kind: WideKind,
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
    /// Whether this row ends in a native soft wrap.
    pub soft_wrapped: bool,
    /// Whether this row continues a native soft wrap from its predecessor.
    pub wrap_continuation: bool,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "ANSI and DEC modes are independently toggled terminal protocol bits"
)]
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
