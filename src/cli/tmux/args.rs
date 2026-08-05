use super::*;

#[derive(Debug, Args)]
pub(in crate::cli) struct TmuxArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: TmuxSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum TmuxSubcommand {
    /// Print a tmux configuration fragment for recording from server startup.
    Config(TmuxConfigArgs),
    /// Enable recording for future panes in a running tmux server.
    Enable(TmuxEnableArgs),
    /// Restore the tmux options saved by enable.
    Disable(TmuxConnectionArgs),
    /// Audit tmux configuration and current pane coverage.
    Status(TmuxStatusArgs),
    /// Show or replace the recording scope for future pane processes.
    Scope(TmuxScopeArgs),
    /// Run one pane command through a full Termtrace-owned PTY recording.
    Run(TmuxRunArgs),
    /// Enforce the configured no-bypass policy for one newly created pane.
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

    /// Permit explicit tmux commands to bypass Termtrace instead of closing their panes.
    #[arg(long)]
    pub(in crate::cli) allow_unrecorded: bool,

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
    #[command(flatten)]
    pub(in crate::cli) connection: TmuxConnectionArgs,

    /// Emit a structured status report.
    #[arg(long)]
    pub(in crate::cli) json: bool,
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
