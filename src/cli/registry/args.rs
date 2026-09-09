use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub(in crate::cli) struct RegistryArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: RegistrySubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum RegistrySubcommand {
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
pub(in crate::cli) struct RegisterArgs {
    /// Friendly session name.
    pub(in crate::cli) name: String,
    /// Existing owner-only canonical session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ResolveArgs {
    /// Friendly session name.
    pub(in crate::cli) name: String,
    /// Emit the complete registry entry as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ListArgs {
    /// Include completed, incomplete, stale, and unhealthy entries.
    #[arg(long)]
    pub(in crate::cli) all: bool,
    /// Emit the complete listing as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct RemoveArgs {
    /// Friendly session name to unbind.
    pub(in crate::cli) name: String,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct GcArgs {
    /// Consider only recordings completed at least this many days ago.
    #[arg(long, default_value_t = 30)]
    pub(in crate::cli) older_than_days: u64,
    /// Always retain this many newest completed unregistered recordings.
    #[arg(long, default_value_t = 20)]
    pub(in crate::cli) keep_latest: u64,
    /// Target bytes for completed managed recordings; overrides age, preserves names/newest.
    #[arg(long)]
    pub(in crate::cli) max_completed_bytes: Option<u64>,
    /// Apply the reported deletions; omission is a read-only dry run.
    #[arg(long)]
    pub(in crate::cli) apply: bool,
}
