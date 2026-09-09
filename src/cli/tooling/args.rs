use crate::cli::parsers::parse_capability_id;
use clap::{Args, Subcommand};
use clap_complete::Shell;
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Args)]
#[command(disable_help_subcommand = true)]
pub(in crate::cli) struct ToolingArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: ToolingSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum ToolingSubcommand {
    /// Run Python or TypeScript client code from managed SDK assets without project files.
    Client(ClientArgs),
    /// Generate JSON Schemas for public events, controls, and artifacts.
    Schemas(SchemaArgs),
    /// Generate a shell completion script from the current command graph.
    Completions(CompletionsArgs),
}

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
pub(in crate::cli) struct ClientArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: ClientCommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum ClientCommand {
    /// Report SDK asset discovery and available managed runtimes.
    Status,
    /// Run a Python file, or `-` for standard input, in an isolated uv environment.
    Python(ClientRunArgs),
    /// Run a TypeScript file, or `-` for standard input, with an installed import mapping.
    Typescript(ClientRunArgs),
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ClientRunArgs {
    /// Client source file, or `-` to read the program from standard input.
    pub(in crate::cli) script: PathBuf,
    /// Arguments passed to the client program after `--`.
    #[arg(last = true, allow_hyphen_values = true)]
    pub(in crate::cli) arguments: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub(in crate::cli) shell: Shell,
}
