use super::*;

#[derive(Debug, Args)]
pub(super) struct RegistryArgs {
    #[command(subcommand)]
    pub(super) command: RegistrySubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum RegistrySubcommand {
    /// Register a friendly name for an existing canonical session.
    Register(RegisterArgs),
    /// Resolve a friendly name to its canonical session directory.
    Resolve(ResolveArgs),
    /// List registered sessions and their derived lifecycle.
    List(ListArgs),
    /// Remove only a friendly registry binding, preserving evidence.
    Remove(RemoveArgs),
    /// Plan or apply safe retention for completed unregistered managed recordings.
    Gc(GcArgs),
}

#[derive(Debug, Args)]
pub(super) struct RegisterArgs {
    /// Friendly session name.
    pub(super) name: String,
    /// Existing owner-only canonical session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
}

#[derive(Debug, Args)]
pub(super) struct ResolveArgs {
    /// Friendly session name.
    pub(super) name: String,
    /// Emit the complete registry entry as JSON.
    #[arg(long)]
    pub(super) json: bool,
}

#[derive(Debug, Args)]
pub(super) struct ListArgs {
    /// Include completed, incomplete, stale, and unhealthy entries.
    #[arg(long)]
    pub(super) all: bool,
    /// Emit the complete listing as JSON.
    #[arg(long)]
    pub(super) json: bool,
}

#[derive(Debug, Args)]
pub(super) struct RemoveArgs {
    /// Friendly session name to unbind.
    pub(super) name: String,
}

#[derive(Debug, Args)]
pub(super) struct GcArgs {
    /// Consider only recordings completed at least this many days ago.
    #[arg(long, default_value_t = 30)]
    pub(super) older_than_days: u64,
    /// Always retain this many newest completed unregistered recordings.
    #[arg(long, default_value_t = 20)]
    pub(super) keep_latest: u64,
    /// Apply the reported deletions; omission is a read-only dry run.
    #[arg(long)]
    pub(super) apply: bool,
}
