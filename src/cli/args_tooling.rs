use super::*;

#[derive(Debug, Args)]
pub(super) struct ToolingArgs {
    #[command(subcommand)]
    pub(super) command: ToolingSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ToolingSubcommand {
    /// Probe built-in and optional host capabilities.
    Doctor(DoctorArgs),
    /// Run Python or TypeScript client code from managed SDK assets without project files.
    Client(ClientArgs),
    /// Serve the versioned language-neutral JSONL process protocol.
    Driver,
    /// Generate JSON Schemas for public events, controls, and artifacts.
    Schemas(SchemaArgs),
    /// Generate a shell completion script from the current command graph.
    Completions(CompletionsArgs),
    /// Report build and runtime component versions.
    Version(VersionArgs),
}

#[derive(Debug, Args)]
pub(super) struct VersionArgs {
    /// Emit stable structured component/build metadata.
    #[arg(long)]
    pub(super) json: bool,
}

#[derive(Debug, Args)]
pub(super) struct DoctorArgs {
    /// Emit the stable structured capability report.
    #[arg(long)]
    pub(super) json: bool,
    /// Comma-separated capabilities that must be ready.
    #[arg(long, value_delimiter = ',', value_parser = parse_capability_id)]
    pub(super) require: Vec<String>,
    /// Include focused remediation for blocked capabilities in text output.
    #[arg(long)]
    pub(super) explain: bool,
}

#[derive(Debug, Args)]
pub(super) struct SchemaArgs {
    /// Destination directory; existing files with generated schema names are replaced.
    #[arg(long)]
    pub(super) out: PathBuf,
}

#[derive(Debug, Args)]
pub(super) struct ClientArgs {
    #[command(subcommand)]
    pub(super) command: ClientCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ClientCommand {
    /// Report SDK asset discovery and available managed runtimes.
    Status,
    /// Run a Python file, or `-` for standard input, in an isolated uv environment.
    Python(ClientRunArgs),
    /// Run a TypeScript file, or `-` for standard input, with an installed import mapping.
    Typescript(ClientRunArgs),
}

#[derive(Debug, Args)]
pub(super) struct ClientRunArgs {
    /// Client source file, or `-` to read the program from standard input.
    pub(super) script: PathBuf,
    /// Arguments passed to the client program after `--`.
    #[arg(last = true, allow_hyphen_values = true)]
    pub(super) arguments: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(super) struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub(super) shell: Shell,
}
