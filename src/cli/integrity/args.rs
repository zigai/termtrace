use crate::cli::analysis::args::PageArgs;
use crate::cli::artifacts::args::BundleVerifyArgs;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;
use termtrace_model::execution::session::VerificationLevel;

#[derive(Debug, Args)]
pub(in crate::cli) struct IntegrityArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: IntegritySubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum IntegritySubcommand {
    /// Verify a completed session's structure and evidence bindings offline.
    Verify(VerifyArgs),
    /// Verify every manifested bundle member against its canonical source.
    VerifyBundle(BundleVerifyArgs),
    /// Create a detached OpenSSH receipt for verified evidence roots.
    Sign(SignArgs),
    /// Copy the longest integrity-checked event prefix without modifying source.
    Recover(RecoverArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum VerifyLevelArg {
    /// Validate session directory structure and manifest integrity.
    Structure,
    /// Validate cryptographic content hashes and event sequence continuity.
    Content,
    /// Validate persistence durability records and fsync boundaries.
    DurabilityRecord,
    /// Validate cryptographic OpenSSH digital signatures against trusted signers.
    Signature,
}

impl From<VerifyLevelArg> for VerificationLevel {
    fn from(value: VerifyLevelArg) -> Self {
        return match value {
            VerifyLevelArg::Structure => Self::Structure,
            VerifyLevelArg::Content => Self::Content,
            VerifyLevelArg::DurabilityRecord => Self::DurabilityRecord,
            VerifyLevelArg::Signature => Self::Signature,
        };
    }
}

#[derive(Debug, Args)]
pub(in crate::cli) struct VerifyArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// Verification depth.
    #[arg(long, value_enum, default_value_t = VerifyLevelArg::Content)]
    pub(in crate::cli) level: VerifyLevelArg,
    /// Emit the stable machine-readable report.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    /// OpenSSH allowed-signers file used to establish signer trust.
    #[arg(long)]
    pub(in crate::cli) allowed_signers: Option<PathBuf>,
    #[command(flatten)]
    pub(in crate::cli) page: PageArgs,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct SignArgs {
    /// Completed canonical session directory.
    pub(in crate::cli) session: PathBuf,
    /// OpenSSH private key, or public key backed by an available signing agent.
    #[arg(long)]
    pub(in crate::cli) key: PathBuf,
    /// Identity that an allowed-signers policy will bind to this key.
    #[arg(long)]
    pub(in crate::cli) signer: String,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct RecoverArgs {
    /// Damaged or truncated source event log.
    #[arg(long, required_unless_present = "session", conflicts_with = "session")]
    pub(in crate::cli) event_log: Option<PathBuf>,
    /// Recover a session directory with its manifest and visual profile; remains incomplete.
    #[arg(long)]
    pub(in crate::cli) session: Option<PathBuf>,
    /// New recovered log path, or session directory with --session.
    #[arg(long)]
    pub(in crate::cli) out: PathBuf,
}
