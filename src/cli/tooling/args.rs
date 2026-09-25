use crate::cli::parsers::parse_capability_id;
use clap::Args;
use clap_complete::Shell;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub(in crate::cli) struct VersionArgs {
    /// Emit stable structured component/build metadata.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct DoctorArgs {
    /// Emit the stable structured capability report.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    /// Comma-separated capabilities that must be ready.
    #[arg(long, value_delimiter = ',', value_parser = parse_capability_id)]
    pub(in crate::cli) require: Vec<String>,
    /// Include focused remediation for blocked capabilities in text output.
    #[arg(long)]
    pub(in crate::cli) explain: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct SchemaArgs {
    /// existing files with generated schema names are replaced.
    #[arg(long)]
    pub(in crate::cli) out: PathBuf,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub(in crate::cli) shell: Shell,
}
