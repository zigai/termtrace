use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{
    artifacts::args::{VideoPresetArg, VideoSourceArg},
    parsers::parse_video_fps,
};

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: TmuxSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum TmuxSubcommand {
    /// Start one aggregate recording with a composite client and pane tracks.
    Record(TmuxRecordArgs),
    /// Report aggregate recording lifecycle, or audit a server when NAME is omitted.
    Status(TmuxStatusArgs),
    /// Stop an aggregate recording and its composite client track.
    Stop(TmuxStopArgs),
    /// Render the aggregate recording's composite client track.
    Video(TmuxVideoArgs),
    /// Bundle the aggregate manifest and every canonical track.
    Bundle(TmuxBundleArgs),
    /// Print a tmux configuration fragment for recording from server startup.
    Config(TmuxConfigArgs),
    /// Enable recording for future panes in a running tmux server.
    Enable(TmuxEnableArgs),
    /// Restore the tmux options saved by enable.
    Disable(TmuxConnectionArgs),
    /// Show or replace the recording scope for future pane processes.
    Scope(TmuxScopeArgs),
    /// Run one pane command through a full Termtrace-owned PTY recording.
    Run(TmuxRunArgs),
    /// Enforce the configured coverage policy for one newly created pane.
    #[command(hide = true)]
    Guard(TmuxGuardArgs),
    /// Audit pane ownership for the lifetime of one tmux server.
    #[command(hide = true)]
    Supervise,
}

#[derive(Clone, Debug, Args)]
pub(in crate::cli) struct TmuxConnectionArgs {
    /// tmux executable.
    #[arg(long, default_value = "tmux")]
    pub(in crate::cli) tmux: PathBuf,

    /// tmux socket name, equivalent to tmux -L.
    #[arg(short = 'L', long, conflicts_with = "socket_path")]
    pub(in crate::cli) socket_name: Option<OsString>,

    /// tmux socket path, equivalent to tmux -S.
    #[arg(short = 'S', long, conflicts_with = "socket_name")]
    pub(in crate::cli) socket_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum TmuxGhosttyProfileMode {
    /// Save the current Ghostty profile when Ghostty is available.
    Auto,
    /// Require and save the current Ghostty profile for every pane.
    Required,
    /// Record semantic evidence without a saved Ghostty profile.
    Off,
}

impl TmuxGhosttyProfileMode {
    pub(in crate::cli) fn as_str(self) -> &'static str {
        return match self {
            Self::Auto => "auto",
            Self::Required => "required",
            Self::Off => "off",
        };
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(in crate::cli) enum TmuxCoverageMode {
    /// Report uncovered panes without terminating them.
    Warn,
    /// Permit Termtrace to terminate uncovered panes after the grace period.
    Strict,
}

impl TmuxCoverageMode {
    pub(in crate::cli) fn as_str(self) -> &'static str {
        return match self {
            Self::Warn => "warn",
            Self::Strict => "strict",
        };
    }
}

#[derive(Clone, Debug, Args)]
pub(in crate::cli) struct TmuxSetupArgs {
    /// Managed Termtrace state root; defaults to the normal per-user state root.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,

    /// Ghostty profile capture policy for new panes.
    #[arg(long, value_enum, default_value_t = TmuxGhosttyProfileMode::Auto)]
    pub(in crate::cli) ghostty_profile: TmuxGhosttyProfileMode,

    /// Ghostty configuration used instead of the default user configuration.
    #[arg(long)]
    pub(in crate::cli) ghostty_config: Option<PathBuf>,

    /// Coverage behavior for unrecorded selected panes.
    #[arg(long, value_enum, default_value_t = TmuxCoverageMode::Warn)]
    pub(in crate::cli) coverage: TmuxCoverageMode,

    /// Aggregate manifest updated by the supervisor.
    #[arg(long, hide = true)]
    pub(in crate::cli) aggregate_manifest: Option<PathBuf>,

    /// Record only panes launched explicitly through `tmux run`.
    #[arg(
        long,
        conflicts_with_all = ["sessions", "windows", "panes"]
    )]
    pub(in crate::cli) explicit_only: bool,

    /// Select one tmux session; repeat to select more sessions.
    #[arg(long = "session", value_name = "SESSION")]
    pub(in crate::cli) sessions: Vec<OsString>,

    /// Select one tmux window; repeat to select more windows.
    #[arg(long = "window", value_name = "WINDOW")]
    pub(in crate::cli) windows: Vec<OsString>,

    /// Select one stable tmux pane ID; repeat to select more panes.
    #[arg(long = "pane", value_name = "PANE")]
    pub(in crate::cli) panes: Vec<OsString>,

    /// Fallback cell width when tmux does not report pixel dimensions.
    #[arg(long, default_value_t = 12)]
    pub(in crate::cli) cell_width: u16,

    /// Fallback cell height when tmux does not report pixel dimensions.
    #[arg(long, default_value_t = 24)]
    pub(in crate::cli) cell_height: u16,

    /// Override the terminal identity; the default inherits tmux's pane TERM.
    #[arg(long)]
    pub(in crate::cli) term: Option<String>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxConfigArgs {
    /// tmux executable used by pane wrappers and enforcement hooks.
    #[arg(long, default_value = "tmux")]
    pub(in crate::cli) tmux: PathBuf,

    #[command(flatten)]
    pub(in crate::cli) setup: TmuxSetupArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxEnableArgs {
    #[command(flatten)]
    pub(in crate::cli) connection: TmuxConnectionArgs,

    #[command(flatten)]
    pub(in crate::cli) setup: TmuxSetupArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxStatusArgs {
    /// Aggregate tmux recording name; omission audits the selected tmux server.
    pub(in crate::cli) recording: Option<String>,

    #[command(flatten)]
    pub(in crate::cli) connection: TmuxConnectionArgs,

    /// Managed state root containing aggregate tmux recordings.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,

    /// Emit a structured status report.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxRecordArgs {
    /// Stable aggregate recording name.
    pub(in crate::cli) name: String,

    /// Start a new tmux server using the generated recording configuration.
    #[arg(long)]
    pub(in crate::cli) new: bool,

    #[command(flatten)]
    pub(in crate::cli) connection: TmuxConnectionArgs,

    #[command(flatten)]
    pub(in crate::cli) setup: TmuxSetupArgs,

    /// Exact tmux client command to record as the composite track.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
    pub(in crate::cli) command: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxStopArgs {
    /// Aggregate tmux recording name.
    pub(in crate::cli) name: String,

    /// Managed state root containing aggregate tmux recordings.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,

    /// Wait for durable completion of the composite track.
    #[arg(long)]
    pub(in crate::cli) wait: bool,

    /// Maximum completion wait in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub(in crate::cli) timeout_ms: u64,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxVideoArgs {
    /// Aggregate tmux recording name.
    pub(in crate::cli) name: String,

    /// New video file.
    #[arg(short = 'o', long)]
    pub(in crate::cli) out: PathBuf,

    /// Managed state root containing aggregate tmux recordings.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = VideoPresetArg::Docs)]
    pub(in crate::cli) preset: VideoPresetArg,

    /// Optional exact video frame rate; presets choose their own default when omitted.
    #[arg(long, value_parser = parse_video_fps)]
    pub(in crate::cli) fps: Option<u16>,

    #[arg(long, value_enum, default_value_t = VideoSourceArg::Semantic)]
    pub(in crate::cli) renderer: VideoSourceArg,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxBundleArgs {
    /// Aggregate tmux recording name.
    pub(in crate::cli) name: String,

    /// New complete aggregate evidence directory.
    #[arg(short = 'o', long)]
    pub(in crate::cli) out: PathBuf,

    /// Managed state root containing aggregate tmux recordings.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxScopeArgs {
    #[command(flatten)]
    pub(in crate::cli) connection: TmuxConnectionArgs,

    /// Record every future pane process.
    #[arg(
        long,
        conflicts_with_all = ["explicit_only", "sessions", "windows", "panes"]
    )]
    pub(in crate::cli) all: bool,

    /// Record only pane processes launched explicitly through `tmux run`.
    #[arg(
        long,
        conflicts_with_all = ["all", "sessions", "windows", "panes"]
    )]
    pub(in crate::cli) explicit_only: bool,

    /// Select one live tmux session target; repeat to select more sessions.
    #[arg(long = "session", value_name = "TARGET")]
    pub(in crate::cli) sessions: Vec<OsString>,

    /// Select one live tmux window target; repeat to select more windows.
    #[arg(long = "window", value_name = "TARGET")]
    pub(in crate::cli) windows: Vec<OsString>,

    /// Select one live tmux pane target; repeat to select more panes.
    #[arg(long = "pane", value_name = "TARGET")]
    pub(in crate::cli) panes: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxRunArgs {
    /// Record the saved default command even when the pane is outside the managed scope.
    #[arg(long, conflicts_with_all = ["shell_command", "command"])]
    pub(in crate::cli) force: bool,

    /// Interpret one exact command string through /bin/sh -c.
    #[arg(long, conflicts_with = "command")]
    pub(in crate::cli) shell_command: Option<OsString>,

    /// Exact executable and arguments; omission restores tmux's saved default.
    #[arg(last = true, allow_hyphen_values = true)]
    pub(in crate::cli) command: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxGuardArgs {
    /// Stable pane ID supplied by the tmux creation hook.
    #[arg(long)]
    pub(in crate::cli) pane: String,
}
