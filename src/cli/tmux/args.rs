use crate::cli::{
    artifacts::args::{VideoPresetArg, VideoSourceArg},
    parsers::parse_video_fps,
};
use clap::{Args, Subcommand, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use std::{ffi::OsString, os::unix::ffi::OsStrExt as _, path::PathBuf};
use termtrace_model::value::ByteString;
use termtrace_tmux::{
    artifact::{TmuxRecordingCoverage, TmuxSocket},
    client::TmuxClient,
    pane::PaneRunRequest,
    scope::ScopeTargets,
    setup::{GhosttyProfileMode, PaneRetention, SetupRequest},
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
    /// Read a bounded page of complete aggregate lifecycle history.
    Events(TmuxEventsArgs),
    /// Read a bounded page of every immutable pane recording incarnation.
    Tracks(TmuxCatalogArgs),
    /// Read historical coverage intervals, including resolved gaps.
    Gaps(TmuxCatalogArgs),
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
    /// Restore the exact tmux launch semantics inside the recorded PTY.
    #[command(hide = true)]
    ExecPane(TmuxExecPaneArgs),
    /// Enforce the configured coverage policy for one newly created pane.
    #[command(hide = true)]
    Guard(TmuxGuardArgs),
    /// Audit pane ownership for the lifetime of one tmux server.
    #[command(hide = true)]
    Supervise,
    /// Restart a failed pane observer while recording remains enabled.
    #[command(hide = true)]
    Watchdog,
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

impl From<TmuxGhosttyProfileMode> for GhosttyProfileMode {
    fn from(mode: TmuxGhosttyProfileMode) -> Self {
        return match mode {
            TmuxGhosttyProfileMode::Auto => Self::Auto,
            TmuxGhosttyProfileMode::Required => Self::Required,
            TmuxGhosttyProfileMode::Off => Self::Off,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum TmuxCoverageMode {
    /// Report uncovered panes without terminating them.
    Warn,
    /// Permit Termtrace to terminate uncovered panes after the grace period.
    Strict,
}

impl From<TmuxCoverageMode> for TmuxRecordingCoverage {
    fn from(mode: TmuxCoverageMode) -> Self {
        return match mode {
            TmuxCoverageMode::Warn => Self::Warn,
            TmuxCoverageMode::Strict => Self::Strict,
        };
    }
}

impl TmuxConnectionArgs {
    pub(super) fn client(&self) -> Result<TmuxClient> {
        let socket = match (&self.socket_name, &self.socket_path) {
            (Some(name), _) => TmuxSocket::Name(ByteString::from(name.as_bytes())),
            (None, Some(path)) => TmuxSocket::Path(ByteString::from(path.as_os_str().as_bytes())),
            (None, None) => TmuxSocket::Default,
        };

        let client = TmuxClient::new(self.tmux.clone(), socket);
        client.require_supported_version().into_diagnostic()?;

        return Ok(client);
    }
}

impl TmuxSetupArgs {
    pub(super) fn request(self) -> SetupRequest {
        return SetupRequest {
            launch_hook: self.launch_hook,
            profile_mode: self.ghostty_profile.into(),
            ghostty_config: self.ghostty_config,
            coverage: self.coverage.into(),
            aggregate_manifest: self.aggregate_manifest,
            explicit_only: self.explicit_only,
            targets: ScopeTargets {
                sessions: self.sessions,
                windows: self.windows,
                panes: self.panes,
            },
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            term: self.term,
            retention: PaneRetention {
                older_than_days: self.retain_days,
                max_bytes: self.retain_bytes,
            },
        };
    }
}

impl From<TmuxRunArgs> for PaneRunRequest {
    fn from(arguments: TmuxRunArgs) -> Self {
        return Self {
            pane_launch: arguments.pane_launch,
            force: arguments.force,
            shell_command: arguments.shell_command,
            command: arguments.command,
        };
    }
}

#[derive(Clone, Debug, Args)]
pub(in crate::cli) struct TmuxSetupArgs {
    /// Intercept every launch and respawn; requires the supplied tmux launch-hook patch.
    #[arg(long)]
    pub(in crate::cli) launch_hook: bool,
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

    /// Delete completed pane recordings older than this many days.
    #[arg(long, value_name = "DAYS")]
    pub(in crate::cli) retain_days: Option<u64>,

    /// Delete the oldest completed pane recordings while their total exceeds this many bytes.
    #[arg(long, value_name = "BYTES")]
    pub(in crate::cli) retain_bytes: Option<u64>,
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
pub(in crate::cli) struct TmuxEventsArgs {
    /// Aggregate recording name.
    pub(in crate::cli) name: String,
    /// Managed state root containing aggregate tmux recordings.
    #[arg(long)]
    pub(in crate::cli) output_root: Option<PathBuf>,
    /// Inclusive first lifecycle sequence.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) from_sequence: u64,
    /// Maximum number of events returned (1-4096), subject to a four-megabyte page limit.
    #[arg(long, default_value_t = 128, value_parser = clap::value_parser!(u16).range(1..=4096))]
    pub(in crate::cli) limit: u16,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxCatalogArgs {
    #[command(flatten)]
    pub(in crate::cli) page: TmuxEventsArgs,
    /// Return only unfinished tracks or unresolved coverage intervals.
    #[arg(long)]
    pub(in crate::cli) active_only: bool,
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

    /// Optional exact video frame rate (24, 30, or 60); presets choose their own default when omitted.
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
    /// Arguments were supplied by the optional tmux pre-exec hook.
    #[arg(long, hide = true, conflicts_with_all = ["force", "shell_command"])]
    pub(in crate::cli) pane_launch: bool,
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
pub(in crate::cli) struct TmuxExecPaneArgs {
    #[arg(last = true, required = true, num_args = 2.., allow_hyphen_values = true)]
    pub(in crate::cli) command: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxGuardArgs {
    /// Stable pane ID supplied by the tmux creation hook.
    #[arg(long)]
    pub(in crate::cli) pane: String,
}
