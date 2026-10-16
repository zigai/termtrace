use clap::{builder::PossibleValuesParser, Args};
use clap_complete::Shell;
use std::path::PathBuf;
use termtrace_model::protocol::capability::CAPABILITY_IDS;

#[derive(Debug, Args)]
pub(in crate::cli) struct VersionArgs {
    /// Emit stable structured component/build metadata.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct DoctorArgs {
    /// Emit the report as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    /// Comma-separated capabilities that must be ready; any other result fails the run.
    #[arg(
        long,
        value_name = "ID,...",
        value_delimiter = ',',
        value_parser = PossibleValuesParser::new(CAPABILITY_IDS)
    )]
    pub(in crate::cli) require: Vec<String>,
    /// Apply the automatic repairs the report lists.
    #[arg(long)]
    pub(in crate::cli) fix: bool,
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
