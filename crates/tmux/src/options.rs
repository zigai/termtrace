//! tmux option names, hooks, and timing shared by every integration role.

use std::time::Duration;

/// Integration version stored in [`VERSION`]; any other value disables every role.
pub const INTEGRATION_VERSION: &str = "1";
/// Reserved index for Termtrace's guarded tmux hooks.
pub const HOOK_INDEX: u16 = 991;
/// Environment variable carrying the tmux executable to pane wrappers and observers.
pub const WRAPPER_BINARY_ENV: &str = "TERMTRACE_TMUX_BINARY";
/// tmux wait channel signaled after every change the supervisor observes.
pub const OBSERVER_CHANNEL: &str = "termtrace-observer";
/// Time a selected pane may run without a wrapper before enforcement.
pub const UNRECORDED_GRACE: Duration = Duration::from_millis(500);
/// Global: integration version.
pub const VERSION: &str = "@termtrace-recording-version";
/// Global: Termtrace executable.
pub const TERMTRACE: &str = "@termtrace-executable";
/// Global: tmux executable.
pub const TMUX: &str = "@termtrace-tmux-executable";
/// Global: managed state root.
pub const OUTPUT_ROOT: &str = "@termtrace-output-root";
/// Global: saved `default-command`.
pub const ORIGINAL_COMMAND: &str = "@termtrace-original-default-command";
/// Global: saved `default-shell`.
pub const ORIGINAL_SHELL: &str = "@termtrace-original-default-shell";
/// Global: whether the patched launch hook is used.
pub const LAUNCH_HOOK: &str = "@termtrace-launch-hook";
/// Global: saved `pane-launch-command`.
pub const ORIGINAL_LAUNCH_HOOK: &str = "@termtrace-original-pane-launch-command";
/// Global: Ghostty profile policy.
pub const PROFILE_MODE: &str = "@termtrace-ghostty-profile";
/// Global: Ghostty configuration path.
pub const GHOSTTY_CONFIG: &str = "@termtrace-ghostty-config";
/// Global: fallback cell width.
pub const CELL_WIDTH: &str = "@termtrace-cell-width";
/// Global: fallback cell height.
pub const CELL_HEIGHT: &str = "@termtrace-cell-height";
/// Global: coverage policy.
pub const COVERAGE: &str = "@termtrace-coverage";
/// Global: aggregate manifest updated by the supervisor.
pub const AGGREGATE_MANIFEST: &str = "@termtrace-aggregate-manifest";
/// Global: terminal identity override.
pub const TERM: &str = "@termtrace-term";
/// Global: age in days after which completed pane recordings are deleted; empty keeps them.
pub const RETAIN_DAYS: &str = "@termtrace-retain-days";
/// Global: byte target for completed pane recordings; empty sets none.
pub const RETAIN_BYTES: &str = "@termtrace-retain-bytes";
/// Global: recording scope policy.
pub const SCOPE: &str = "@termtrace-scope";
/// Pane: published recording session path.
pub const PANE_SESSION: &str = "@termtrace-session";
/// Pane: process that predates enabling.
pub const GRANDFATHERED_PID: &str = "@termtrace-grandfathered-pid";
/// Pane: process selected for recording.
pub const SELECTED_PID: &str = "@termtrace-selected-pid";
/// Pane: process excluded from recording.
pub const EXCLUDED_PID: &str = "@termtrace-excluded-pid";
/// Hooks that run the pane guard.
pub const GUARDED_HOOKS: &[&str] = &[
    "after-new-session",
    "after-new-window",
    "after-split-window",
];
/// Hooks that signal [`OBSERVER_CHANNEL`] after tmux changes panes, windows, or sessions.
pub const OBSERVED_HOOKS: &[&str] = &[
    "client-session-changed",
    "pane-died",
    "pane-exited",
    "pane-mode-changed",
    "session-closed",
    "session-created",
    "session-renamed",
    "session-window-changed",
    "window-layout-changed",
    "window-linked",
    "window-pane-changed",
    "window-renamed",
    "window-unlinked",
];
/// Global options owned by the integration and removed by disable.
pub const RESERVED: &[&str] = &[
    VERSION,
    TERMTRACE,
    TMUX,
    OUTPUT_ROOT,
    ORIGINAL_COMMAND,
    ORIGINAL_SHELL,
    LAUNCH_HOOK,
    ORIGINAL_LAUNCH_HOOK,
    PROFILE_MODE,
    GHOSTTY_CONFIG,
    COVERAGE,
    AGGREGATE_MANIFEST,
    CELL_WIDTH,
    CELL_HEIGHT,
    TERM,
    RETAIN_DAYS,
    RETAIN_BYTES,
    SCOPE,
];
/// Pane options owned by the integration and removed by disable.
pub const PANE_MARKS: &[&str] = &[GRANDFATHERED_PID, SELECTED_PID, EXCLUDED_PID];
