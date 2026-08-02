use super::*;

#[derive(Debug, Args)]
pub(super) struct GeometryProbeArgs {
    /// New file receiving the measured terminal geometry.
    #[arg(long)]
    pub(super) out: PathBuf,
}

#[derive(Debug, Args)]
pub(super) struct RecordArgs {
    /// New owner-only session directory.
    #[arg(long)]
    pub(super) out: PathBuf,

    /// Do not forward standard input to the child PTY.
    #[arg(long)]
    pub(super) no_input: bool,

    /// Friendly invocation context supplied by detached start.
    #[arg(long, hide = true)]
    pub(super) session_alias: Option<String>,

    /// Completed predecessor used to construct an exact restart successor.
    #[arg(long, hide = true)]
    pub(super) restart_source: Option<PathBuf>,

    /// Base64-safe JSON restart override descriptor supplied by the supervisor.
    #[arg(long, hide = true)]
    pub(super) restart_overrides: Option<String>,

    /// Internal source kind for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(super) import_source_kind: Option<String>,
    /// Internal exact byte count for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(super) import_source_byte_count: Option<u64>,
    /// Internal exact BLAKE3 for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(super) import_source_blake3: Option<String>,
    /// Internal original source path bytes for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(super) import_source: Option<OsString>,

    #[command(flatten)]
    pub(super) recording: RecordingArgs,
}

#[derive(Debug, Args)]
pub(super) struct StartArgs {
    /// New friendly session name.
    pub(super) name: String,

    /// Maximum time to wait for manifest and control readiness.
    #[arg(long, default_value_t = 10_000)]
    pub(super) startup_timeout_ms: u64,

    #[command(flatten)]
    pub(super) recording: RecordingArgs,
}

#[derive(Debug, Args)]
pub(super) struct RestartArgs {
    /// Existing friendly session name.
    pub(super) name: String,

    /// Maximum time for predecessor completion and successor readiness.
    #[arg(long, default_value_t = 10_000)]
    pub(super) timeout_ms: u64,

    /// Override the successor working directory.
    #[arg(long)]
    pub(super) cwd: Option<PathBuf>,
    /// Override terminal columns while retaining the predecessor when omitted.
    #[arg(long)]
    pub(super) cols: Option<u16>,
    /// Override terminal rows while retaining the predecessor when omitted.
    #[arg(long)]
    pub(super) rows: Option<u16>,
    /// Override cell width in pixels.
    #[arg(long)]
    pub(super) cell_width: Option<u16>,
    /// Override cell height in pixels.
    #[arg(long)]
    pub(super) cell_height: Option<u16>,
    /// Override the terminal identity exposed to the child.
    #[arg(long)]
    pub(super) term: Option<String>,
    /// Override the canonical output persistence policy.
    #[arg(long, value_enum)]
    pub(super) durability: Option<DurabilityModeArg>,
    /// Override the bounded maximum unsynchronized interval.
    #[arg(long)]
    pub(super) durability_max_interval_ms: Option<u64>,
    /// Override the bounded maximum unsynchronized byte count.
    #[arg(long)]
    pub(super) durability_max_uncommitted_bytes: Option<u64>,
    /// Override the execution provenance policy.
    #[arg(long, value_enum)]
    pub(super) provenance: Option<ProvenancePolicyArg>,
    /// Override custom environment rules.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    pub(super) provenance_environment: Vec<(String, ProvenanceCapture)>,
    /// Owner-only JSON child-environment override; values never enter recorder arguments.
    #[arg(long)]
    pub(super) environment_spec: Option<PathBuf>,
    /// Permit a supplied environment specification to differ from the predecessor commitment.
    #[arg(long, requires = "environment_spec")]
    pub(super) allow_environment_change: bool,

    /// Override the soft non-truncating output budget.
    #[arg(long, conflicts_with = "clear_output_byte_budget")]
    pub(super) output_byte_budget: Option<u64>,
    /// Remove the predecessor's output budget.
    #[arg(long)]
    pub(super) clear_output_byte_budget: bool,
    /// Override the executable and arguments; options must precede the executable.
    #[arg(num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
    pub(super) command: Vec<OsString>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestartOverrides {
    pub(super) cwd: Option<ByteString>,
    pub(super) cols: Option<u16>,
    pub(super) rows: Option<u16>,
    pub(super) cell_width: Option<u16>,
    pub(super) cell_height: Option<u16>,
    pub(super) term: Option<String>,
    pub(super) durability: Option<DurabilityModeArg>,
    pub(super) durability_max_interval_ms: Option<u64>,
    pub(super) durability_max_uncommitted_bytes: Option<u64>,
    pub(super) provenance: Option<ProvenancePolicyArg>,
    pub(super) provenance_environment: Vec<(String, ProvenanceCapture)>,
    pub(super) output_byte_budget: Option<u64>,
    pub(super) clear_output_byte_budget: bool,
    pub(super) allow_environment_change: bool,
    pub(super) command: Option<Vec<ByteString>>,
}

#[derive(Debug, Args)]
pub(super) struct RecordingArgs {
    /// Child working directory; defaults to the caller's current directory.
    #[arg(long)]
    pub(super) cwd: Option<PathBuf>,

    /// Geometry selection policy.
    #[arg(long, value_enum, default_value_t = GeometryMode::Manual)]
    pub(super) geometry: GeometryMode,

    /// Intended capture viewport as WIDTHxHEIGHT; required for fitted geometry.
    #[arg(long, value_parser = parse_capture_target)]
    pub(super) viewport: Option<CaptureTarget>,

    /// Ghostty config file used instead of the default user config.
    #[arg(long)]
    pub(super) ghostty_config: Option<PathBuf>,

    /// Override the regular Ghostty font family.
    #[arg(long)]
    pub(super) font_family: Option<String>,

    /// Override the Ghostty font size in points.
    #[arg(long, value_parser = parse_font_size)]
    pub(super) font_size: Option<String>,

    /// Override symmetric Ghostty horizontal padding in pixels.
    #[arg(long)]
    pub(super) padding_x: Option<u16>,

    /// Override symmetric Ghostty vertical padding in pixels.
    #[arg(long)]
    pub(super) padding_y: Option<u16>,

    /// Override the Ghostty foreground color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    pub(super) foreground: Option<String>,

    /// Override the Ghostty background color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    pub(super) background: Option<String>,

    /// Terminal columns used by manual geometry.
    #[arg(long, default_value_t = 160)]
    pub(super) cols: u16,

    /// Terminal rows used by manual geometry.
    #[arg(long, default_value_t = 45)]
    pub(super) rows: u16,

    /// Manual cell width, or inherited-geometry fallback, in pixels.
    #[arg(long, default_value_t = 12)]
    pub(super) cell_width: u16,

    /// Manual cell height, or inherited-geometry fallback, in pixels.
    #[arg(long, default_value_t = 24)]
    pub(super) cell_height: u16,

    /// Terminal identity exposed to the child.
    #[arg(long, default_value = "xterm-ghostty")]
    pub(super) term: String,

    /// Owner-only JSON child-environment specification; values never enter recorder arguments.
    #[arg(long)]
    pub(super) environment_spec: Option<PathBuf>,

    /// Read the JSON environment specification from stdin (requires --no-input for record).
    #[arg(long, conflicts_with = "environment_spec")]
    pub(super) environment_spec_stdin: bool,

    /// Begin with an empty child environment (combine with --copy-env as needed).
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(super) clear_env: bool,

    /// Copy one named recorder variable into the child environment.
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(super) copy_env: Vec<String>,

    /// Remove one named variable from an inherited child environment.
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(super) unset_env: Vec<String>,

    /// Apply deterministic common color-environment behavior.
    #[arg(
        long,
        value_enum,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(super) color_policy: Option<ColorPolicyArg>,

    /// Soft PTY output budget in bytes; all observed bytes remain canonical evidence.
    #[arg(long)]
    pub(super) output_byte_budget: Option<u64>,

    /// Canonical output persistence policy.
    #[arg(long, value_enum, default_value_t = DurabilityModeArg::Bounded)]
    pub(super) durability: DurabilityModeArg,

    /// Maximum unsynchronized interval in bounded mode.
    #[arg(long, default_value_t = 50)]
    pub(super) durability_max_interval_ms: u64,

    /// Maximum unsynchronized PTY bytes in bounded mode.
    #[arg(long, default_value_t = 65_536)]
    pub(super) durability_max_uncommitted_bytes: u64,

    /// Execution provenance collection policy.
    #[arg(long, value_enum, default_value_t = ProvenancePolicyArg::Minimal)]
    pub(super) provenance: ProvenancePolicyArg,

    /// Custom environment rule as NAME=value|hash|name|not-captured.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    pub(super) provenance_environment: Vec<(String, ProvenanceCapture)>,

    /// Executable and arguments; options must precede the executable.
    #[arg(
        required = true,
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(super) command: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum GeometryMode {
    Manual,
    Inherit,
    Fit,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
pub(super) enum DurabilityModeArg {
    Strict,
    Bounded,
    Throughput,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
pub(super) enum ProvenancePolicyArg {
    Minimal,
    Reproducible,
    Custom,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum ColorPolicyArg {
    Auto,
    Always,
    Never,
}

impl From<ColorPolicyArg> for ColorPolicy {
    fn from(value: ColorPolicyArg) -> Self {
        return match value {
            ColorPolicyArg::Auto => Self::Auto,
            ColorPolicyArg::Always => Self::Always,
            ColorPolicyArg::Never => Self::Never,
        };
    }
}

impl From<ProvenancePolicyArg> for ProvenancePolicyKind {
    fn from(value: ProvenancePolicyArg) -> Self {
        return match value {
            ProvenancePolicyArg::Minimal => Self::Minimal,
            ProvenancePolicyArg::Reproducible => Self::Reproducible,
            ProvenancePolicyArg::Custom => Self::Custom,
        };
    }
}

impl DurabilityModeArg {
    pub(super) fn policy(
        self,
        maximum_interval_ms: u64,
        maximum_uncommitted_bytes: u64,
    ) -> DurabilityPolicy {
        return match self {
            Self::Strict => DurabilityPolicy {
                mode: DurabilityMode::Strict,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
            Self::Bounded => DurabilityPolicy {
                mode: DurabilityMode::Bounded,
                maximum_interval_ms: Some(maximum_interval_ms),
                maximum_uncommitted_bytes: Some(maximum_uncommitted_bytes),
            },
            Self::Throughput => DurabilityPolicy {
                mode: DurabilityMode::Throughput,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
        };
    }
}
