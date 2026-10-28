//! The one set of defaults every Termtrace interface uses.

use std::time::Duration;

/// Initial terminal columns for a fresh recording.
pub const COLS: u16 = 160;
/// Initial terminal rows for a fresh recording.
pub const ROWS: u16 = 45;
/// Nominal cell width in pixels used to derive pixel geometry.
pub const CELL_WIDTH: u16 = 12;
/// Nominal cell height in pixels used to derive pixel geometry.
pub const CELL_HEIGHT: u16 = 24;
/// How long a fresh recorder may take to answer its control socket.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a screen wait, stable capture, or action postcondition may run.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause between locator wait polls.
pub const LOCATOR_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long the presented screen must stay unchanged to count as idle.
pub const IDLE_STABLE_FOR: Duration = Duration::from_millis(500);
/// How long an idle condition waits for output or a changed screen before reporting
/// `unchanged`.
pub const REACTION_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a native replay worker may take to start its terminal and display.
pub const NATIVE_REPLAY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a stop waits for validated completion.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long an unreachable live session may take to publish completion before the
/// failure is reported instead; the one live-to-completed transition window.
pub const COMPLETION_GRACE: Duration = Duration::from_secs(2);
/// Garbage collection deletes completed sessions older than this many days.
pub const GC_OLDER_THAN_DAYS: u64 = 30;
/// Garbage collection always keeps this many newest completed sessions.
pub const GC_KEEP_LATEST: u64 = 20;
