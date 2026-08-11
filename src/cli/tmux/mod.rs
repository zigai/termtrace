use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    os::unix::process::CommandExt as _,
    process, str,
};

use super::*;

mod aggregate;
mod args;
mod client;
mod recording;
mod scope;
mod setup;
mod supervisor;

use aggregate::*;
pub(super) use aggregate::{run_tmux_bundle, run_tmux_record, run_tmux_stop, run_tmux_video};
pub(super) use args::*;
use client::*;
pub(super) use recording::run_tmux_pane;
pub(super) use scope::run_tmux_scope;
use scope::*;
use setup::*;
pub(super) use setup::{run_tmux_config, run_tmux_disable, run_tmux_enable};
use supervisor::*;
pub(super) use supervisor::{run_tmux_guard, run_tmux_status, run_tmux_supervisor};

const TMUX_INTEGRATION_VERSION: &str = "1";
const TMUX_HOOK_INDEX: u16 = 991;
const TMUX_WRAPPER_BINARY_ENV: &str = "TERMTRACE_TMUX_BINARY";

const OPTION_VERSION: &str = "@termtrace-recording-version";
const OPTION_TERMTRACE: &str = "@termtrace-executable";
const OPTION_TMUX: &str = "@termtrace-tmux-executable";
const OPTION_OUTPUT_ROOT: &str = "@termtrace-output-root";
const OPTION_ORIGINAL_COMMAND: &str = "@termtrace-original-default-command";
const OPTION_ORIGINAL_SHELL: &str = "@termtrace-original-default-shell";
const OPTION_PROFILE_MODE: &str = "@termtrace-ghostty-profile";
const OPTION_GHOSTTY_CONFIG: &str = "@termtrace-ghostty-config";
const OPTION_ALLOW_UNRECORDED: &str = "@termtrace-allow-unrecorded";
const OPTION_CELL_WIDTH: &str = "@termtrace-cell-width";
const OPTION_COVERAGE: &str = "@termtrace-coverage";
const OPTION_AGGREGATE_MANIFEST: &str = "@termtrace-aggregate-manifest";
const OPTION_CELL_HEIGHT: &str = "@termtrace-cell-height";
const OPTION_TERM: &str = "@termtrace-term";
const OPTION_SCOPE: &str = "@termtrace-scope";
const OPTION_PANE_SESSION: &str = "@termtrace-session";
const OPTION_GRANDFATHERED_PID: &str = "@termtrace-grandfathered-pid";
const OPTION_SELECTED_PID: &str = "@termtrace-selected-pid";
const OPTION_EXCLUDED_PID: &str = "@termtrace-excluded-pid";
const OPTION_SUPERVISOR_PID: &str = "@termtrace-supervisor-pid";
const SUPERVISOR_LOCK: &str = "termtrace-recording-supervisor";
const SUPERVISOR_INTERVAL: Duration = Duration::from_millis(250);
const UNRECORDED_GRACE: Duration = Duration::from_millis(500);

const GUARDED_HOOKS: &[&str] = &[
    "after-new-session",
    "after-new-window",
    "after-split-window",
];

const RESERVED_OPTIONS: &[&str] = &[
    OPTION_VERSION,
    OPTION_TERMTRACE,
    OPTION_TMUX,
    OPTION_OUTPUT_ROOT,
    OPTION_ORIGINAL_COMMAND,
    OPTION_ORIGINAL_SHELL,
    OPTION_PROFILE_MODE,
    OPTION_GHOSTTY_CONFIG,
    OPTION_ALLOW_UNRECORDED,
    OPTION_COVERAGE,
    OPTION_AGGREGATE_MANIFEST,
    OPTION_CELL_WIDTH,
    OPTION_CELL_HEIGHT,
    OPTION_TERM,
    OPTION_SCOPE,
    OPTION_SUPERVISOR_PID,
];
