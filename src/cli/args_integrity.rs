use super::*;

#[derive(Debug, Args)]
pub(super) struct IntegrityArgs {
    #[command(subcommand)]
    pub(super) command: IntegritySubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IntegritySubcommand {
    /// Verify a completed session's structure and evidence bindings offline.
    Verify(VerifyArgs),
    /// Create a detached OpenSSH receipt for verified evidence roots.
    Sign(SignArgs),
    /// Copy the longest integrity-checked event prefix without modifying source.
    Recover(RecoverArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum VerifyLevelArg {
    Structure,
    Content,
    DurabilityRecord,
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
pub(super) struct VerifyArgs {
    /// Completed canonical session directory.
    pub(super) session: PathBuf,
    /// Verification depth.
    #[arg(long, value_enum, default_value_t = VerifyLevelArg::Content)]
    pub(super) level: VerifyLevelArg,
    /// Emit the stable machine-readable report.
    #[arg(long)]
    pub(super) json: bool,
    /// OpenSSH allowed-signers file used to establish signer trust.
    #[arg(long)]
    pub(super) allowed_signers: Option<PathBuf>,
    #[command(flatten)]
    pub(super) page: PageArgs,
}

#[derive(Debug, Args)]
pub(super) struct SignArgs {
    /// Completed canonical session directory.
    pub(super) session: PathBuf,
    /// OpenSSH private key, or public key backed by an available signing agent.
    #[arg(long)]
    pub(super) key: PathBuf,
    /// Identity that an allowed-signers policy will bind to this key.
    #[arg(long)]
    pub(super) signer: String,
}

#[derive(Debug, Args)]
pub(super) struct RecoverArgs {
    /// Damaged or truncated source event log.
    #[arg(long)]
    pub(super) event_log: PathBuf,
    /// New recovered event-log path in an owner-only directory.
    #[arg(long)]
    pub(super) out: PathBuf,
}
