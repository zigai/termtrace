use crate::cli::parsers::{
    parse_bounded_screen_regex, parse_capture_target, parse_font_size, parse_idle_interval,
    parse_provenance_rule, parse_rgb_color, parse_wait_deadline_duration,
};
use clap::{Args, Parser, ValueEnum};
use std::{ffi::OsString, path::PathBuf, time::Duration};
use termtrace_media::helper;
use termtrace_model::execution::session::RecordingFailurePolicy;
use termtrace_model::{
    execution::environment::ColorPolicy,
    execution::event_log::{DurabilityMode, DurabilityPolicy},
    execution::provenance::{ProvenanceCapture, ProvenancePolicyKind},
    execution::session::CaptureTarget,
    value::ByteString,
};
use termtrace_sdk::defaults;

#[derive(Debug, Args)]
pub(in crate::cli) struct GeometryProbeArgs {
    /// New file receiving the measured terminal geometry.
    #[arg(long = helper::geometry_probe::OUT)]
    pub(in crate::cli) out: PathBuf,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct RecordArgs {
    /// Acquire original command pixels in a private Ghostty display, with video and final PNG.
    #[arg(long, conflicts_with_all = ["no_input", "restart_source", "environment_spec_stdin", "import_source_kind", "on_recording_error", "detach"])]
    pub(in crate::cli) original_native: bool,
    /// Original capture rate: 24, 30, or 60 frames per second.
    #[arg(long, requires = "original_native")]
    pub(in crate::cli) original_fps: Option<u16>,
    /// Original pixel source: a private display, or a portal-selected Wayland window.
    #[arg(long, requires = "original_native", value_enum)]
    pub(in crate::cli) original_backend: Option<OriginalBackendArg>,
    /// Internal commitment for an already-resolved original capture profile.
    #[arg(long, hide = true, requires = "ghostty_config")]
    pub(in crate::cli) resolved_ghostty_profile_provenance: Option<String>,
    /// Behavior after a recorder error (default: preserve interactive input, stop headless jobs).
    #[arg(long, value_enum)]
    pub(in crate::cli) on_recording_error: Option<RecordingFailurePolicyArg>,
    /// New owner-only session directory.
    #[arg(long, required_unless_present = "detach")]
    pub(in crate::cli) out: Option<PathBuf>,

    /// Run session in background (optionally under this friendly alias name).
    #[arg(short = 'd', long = "detach", num_args = 0..=1, default_missing_value = "")]
    pub(in crate::cli) detach: Option<String>,
    /// Maximum time to wait for control readiness in milliseconds (1+); defaults to the shared
    /// startup timeout.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) startup_timeout_ms: Option<u64>,
    /// Wait until the first presented screen remains unchanged for this duration, or for the
    /// default idle interval when no value is given.
    #[arg(
        long,
        value_parser = parse_idle_interval,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "",
        conflicts_with_all = ["until_screen", "until_screen_regex"]
    )]
    pub(in crate::cli) until_idle: Option<Duration>,
    /// Wait until the first presented screen contains this text.
    #[arg(long, conflicts_with_all = ["until_idle", "until_screen_regex"])]
    pub(in crate::cli) until_screen: Option<String>,
    /// Wait until the first presented screen matches this bounded regular expression.
    #[arg(long, conflicts_with_all = ["until_idle", "until_screen"], value_parser = parse_bounded_screen_regex)]
    pub(in crate::cli) until_screen_regex: Option<String>,
    /// Maximum time to wait for matching presented state; defaults to the shared wait timeout.
    #[arg(long, value_parser = parse_wait_deadline_duration)]
    pub(in crate::cli) timeout: Option<Duration>,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,

    /// Do not forward standard input to the child PTY.
    #[arg(long)]
    pub(in crate::cli) no_input: bool,

    /// Friendly invocation context supplied by detached start.
    #[arg(long, hide = true)]
    pub(in crate::cli) session_alias: Option<String>,

    /// Completed predecessor used to construct an exact restart successor.
    #[arg(long, hide = true)]
    pub(in crate::cli) restart_source: Option<PathBuf>,

    /// Base64-safe JSON restart override descriptor supplied by the supervisor.
    #[arg(long, hide = true)]
    pub(in crate::cli) restart_overrides: Option<String>,

    /// Internal source kind for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(in crate::cli) import_source_kind: Option<String>,
    /// Internal exact byte count for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(in crate::cli) import_source_byte_count: Option<u64>,
    /// Internal exact BLAKE3 for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(in crate::cli) import_source_blake3: Option<String>,
    /// Internal original source path bytes for a chain-bound ANSI import.
    #[arg(long, hide = true)]
    pub(in crate::cli) import_source: Option<OsString>,

    #[command(flatten)]
    pub(in crate::cli) recording: RecordingArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum OriginalBackendArg {
    /// Acquire pixels from a private headless X11 display.
    PrivateX11,
    /// Acquire pixels from a portal-selected Wayland window.
    WaylandPortal,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum RecordingFailurePolicyArg {
    /// Preserve interactive input after a recorder error.
    Preserve,
    /// Terminate recording immediately upon recorder error.
    Stop,
}

impl From<RecordingFailurePolicyArg> for RecordingFailurePolicy {
    fn from(policy: RecordingFailurePolicyArg) -> Self {
        return match policy {
            RecordingFailurePolicyArg::Preserve => Self::Preserve,
            RecordingFailurePolicyArg::Stop => Self::Stop,
        };
    }
}

#[derive(Debug, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Environment-change permission and clearing each resource limit are independent CLI options"
)]
pub(in crate::cli) struct RestartArgs {
    /// Existing friendly session name.
    pub(in crate::cli) name: String,

    /// Maximum time for predecessor completion and successor readiness in milliseconds (1+);
    /// defaults to the shared startup timeout.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) timeout_ms: Option<u64>,
    /// Override the successor working directory.
    #[arg(long)]
    pub(in crate::cli) cwd: Option<PathBuf>,
    /// Override terminal columns while retaining the predecessor when omitted (1-65535).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cols: Option<u16>,
    /// Override terminal rows while retaining the predecessor when omitted (1-65535).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) rows: Option<u16>,
    /// Override cell width in pixels (1-65535).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cell_width: Option<u16>,
    /// Override cell height in pixels (1-65535).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cell_height: Option<u16>,
    #[arg(long)]
    pub(in crate::cli) term: Option<String>,
    /// Override the canonical output persistence policy.
    #[arg(long, value_enum)]
    pub(in crate::cli) durability: Option<DurabilityModeArg>,
    /// Override the bounded maximum unsynchronized interval in milliseconds (1+).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) durability_max_interval_ms: Option<u64>,
    /// Override the bounded maximum unsynchronized byte count (1+).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) durability_max_uncommitted_bytes: Option<u64>,
    #[arg(long, value_enum)]
    pub(in crate::cli) provenance: Option<ProvenancePolicyArg>,
    /// Override custom environment rules.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    pub(in crate::cli) provenance_environment: Vec<(String, ProvenanceCapture)>,
    /// Owner-only JSON child-environment override; values never enter recorder arguments.
    #[arg(long)]
    pub(in crate::cli) environment_spec: Option<PathBuf>,
    /// Permit a supplied environment specification to differ from the predecessor commitment.
    #[arg(long, requires = "environment_spec")]
    pub(in crate::cli) allow_environment_change: bool,

    /// Override the soft non-truncating output budget.
    #[arg(long, conflicts_with = "clear_output_byte_budget")]
    pub(in crate::cli) output_byte_budget: Option<u64>,
    /// Remove the predecessor's output budget.
    #[arg(long)]
    pub(in crate::cli) clear_output_byte_budget: bool,

    /// Override the maximum wall-clock session duration.
    #[arg(long, value_parser = parse_wait_deadline_duration, conflicts_with = "clear_max_duration")]
    pub(in crate::cli) max_duration: Option<Duration>,
    /// Remove the predecessor's maximum session duration limit.
    #[arg(long)]
    pub(in crate::cli) clear_max_duration: bool,

    /// Override the maximum idle duration without activity.
    #[arg(long, value_parser = parse_wait_deadline_duration, conflicts_with = "clear_idle_timeout")]
    pub(in crate::cli) idle_timeout: Option<Duration>,
    /// Remove the predecessor's idle timeout.
    #[arg(long)]
    pub(in crate::cli) clear_idle_timeout: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
    #[arg(num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
    pub(in crate::cli) command: Vec<OsString>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Serialized restart overrides preserve independent environment permission and resource-limit clearing options"
)]
pub(in crate::cli) struct RestartOverrides {
    pub(in crate::cli) cwd: Option<ByteString>,
    pub(in crate::cli) cols: Option<u16>,
    pub(in crate::cli) rows: Option<u16>,
    pub(in crate::cli) cell_width: Option<u16>,
    pub(in crate::cli) cell_height: Option<u16>,
    pub(in crate::cli) term: Option<String>,
    pub(in crate::cli) durability: Option<DurabilityModeArg>,
    pub(in crate::cli) durability_max_interval_ms: Option<u64>,
    pub(in crate::cli) durability_max_uncommitted_bytes: Option<u64>,
    pub(in crate::cli) provenance: Option<ProvenancePolicyArg>,
    pub(in crate::cli) provenance_environment: Vec<(String, ProvenanceCapture)>,
    pub(in crate::cli) output_byte_budget: Option<u64>,
    pub(in crate::cli) clear_output_byte_budget: bool,
    pub(in crate::cli) max_duration_ms: Option<u64>,
    pub(in crate::cli) clear_max_duration: bool,
    pub(in crate::cli) idle_timeout_ms: Option<u64>,
    pub(in crate::cli) clear_idle_timeout: bool,
    pub(in crate::cli) allow_environment_change: bool,
    pub(in crate::cli) command: Option<Vec<ByteString>>,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct RecordingArgs {
    /// Child working directory; defaults to the caller's current directory.
    #[arg(long)]
    pub(in crate::cli) cwd: Option<PathBuf>,

    /// Geometry selection policy.
    #[arg(long, value_enum, default_value_t = GeometryMode::Manual)]
    pub(in crate::cli) geometry: GeometryMode,

    /// Intended capture viewport as WIDTHxHEIGHT; required for fitted geometry.
    #[arg(long, value_parser = parse_capture_target)]
    pub(in crate::cli) viewport: Option<CaptureTarget>,

    /// Ghostty config file used instead of the default user config.
    #[arg(long)]
    pub(in crate::cli) ghostty_config: Option<PathBuf>,

    /// Override the regular Ghostty font family.
    #[arg(long)]
    pub(in crate::cli) font_family: Option<String>,

    /// Override the Ghostty font size in points.
    #[arg(long, value_parser = parse_font_size)]
    pub(in crate::cli) font_size: Option<String>,

    /// Override symmetric Ghostty horizontal padding in pixels.
    #[arg(long)]
    pub(in crate::cli) padding_x: Option<u16>,

    /// Override symmetric Ghostty vertical padding in pixels.
    #[arg(long)]
    pub(in crate::cli) padding_y: Option<u16>,

    /// Override the Ghostty foreground color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    pub(in crate::cli) foreground: Option<String>,

    /// Override the Ghostty background color as #RRGGBB.
    #[arg(long, value_parser = parse_rgb_color)]
    pub(in crate::cli) background: Option<String>,

    /// Terminal columns used by manual geometry (1-65535).
    #[arg(long, default_value_t = defaults::COLS, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cols: u16,

    /// Terminal rows used by manual geometry (1-65535).
    #[arg(long, default_value_t = defaults::ROWS, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) rows: u16,

    /// Manual cell width, or inherited-geometry fallback, in pixels (1-65535).
    #[arg(long, default_value_t = defaults::CELL_WIDTH, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cell_width: u16,

    /// Manual cell height, or inherited-geometry fallback, in pixels (1-65535).
    #[arg(long, default_value_t = defaults::CELL_HEIGHT, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cell_height: u16,
    #[arg(long, default_value = "xterm-ghostty")]
    pub(in crate::cli) term: String,

    /// Owner-only JSON child-environment specification; values never enter recorder arguments.
    #[arg(long)]
    pub(in crate::cli) environment_spec: Option<PathBuf>,

    /// Read the JSON environment specification from stdin (requires --no-input for record).
    #[arg(long, conflicts_with = "environment_spec")]
    pub(in crate::cli) environment_spec_stdin: bool,

    /// Begin with an empty child environment (combine with --copy-env as needed).
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(in crate::cli) clear_env: bool,

    /// Copy one named recorder variable into the child environment.
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(in crate::cli) copy_env: Vec<String>,

    /// Remove one named variable from an inherited child environment.
    #[arg(
        long,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(in crate::cli) unset_env: Vec<String>,

    /// Apply deterministic common color-environment behavior.
    #[arg(
        long,
        value_enum,
        conflicts_with_all = ["environment_spec", "environment_spec_stdin"]
    )]
    pub(in crate::cli) color_policy: Option<ColorPolicyArg>,

    /// Soft PTY output budget in bytes; all observed bytes remain canonical evidence.
    #[arg(long)]
    pub(in crate::cli) output_byte_budget: Option<u64>,

    /// Maximum session duration before termination is triggered (e.g. 1h, 30m).
    #[arg(long, value_parser = parse_wait_deadline_duration)]
    pub(in crate::cli) max_duration: Option<Duration>,

    /// Maximum idle duration without activity before termination is triggered (e.g. 15m, 60s).
    #[arg(long, value_parser = parse_wait_deadline_duration)]
    pub(in crate::cli) idle_timeout: Option<Duration>,
    /// Canonical output persistence policy.
    #[arg(long, value_enum, default_value_t = DurabilityModeArg::Bounded)]
    pub(in crate::cli) durability: DurabilityModeArg,

    /// Maximum unsynchronized interval in bounded mode in milliseconds (default: 50, 1+).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) durability_max_interval_ms: Option<u64>,

    /// Maximum unsynchronized PTY bytes in bounded mode (default: 65536, 1+).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(in crate::cli) durability_max_uncommitted_bytes: Option<u64>,
    /// Execution provenance collection policy.
    #[arg(long, value_enum, default_value_t = ProvenancePolicyArg::Minimal)]
    pub(in crate::cli) provenance: ProvenancePolicyArg,

    /// Custom environment rule as NAME=value|hash|name|not-captured.
    #[arg(long = "provenance-env", value_parser = parse_provenance_rule)]
    pub(in crate::cli) provenance_environment: Vec<(String, ProvenanceCapture)>,

    /// Executable and arguments; options must precede the executable.
    #[arg(
        required = true,
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(in crate::cli) command: Vec<OsString>,
}

impl RecordingArgs {
    /// Recording settings for `command` exactly as `record` parses them with no options.
    pub(in crate::cli) fn defaults_for(command: &[OsString]) -> Result<Self, clap::Error> {
        #[derive(Parser)]
        struct Defaults {
            #[command(flatten)]
            recording: RecordingArgs,
        }

        let arguments = [OsString::from("termtrace"), OsString::from("--")]
            .into_iter()
            .chain(command.iter().cloned());

        return Defaults::try_parse_from(arguments).map(|defaults| return defaults.recording);
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum GeometryMode {
    /// Use manually specified columns, rows, and cell dimensions.
    Manual,
    /// Inherit terminal geometry from the calling terminal emulator.
    Inherit,
    /// Fit terminal geometry to the intended capture viewport.
    Fit,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
pub(in crate::cli) enum DurabilityModeArg {
    /// Synchronize every canonical event to disk before processing next.
    Strict,
    /// Synchronize events periodically within bounded time and byte limits.
    Bounded,
    /// Maximize throughput by allowing asynchronous uncommitted event buffering.
    Throughput,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, ValueEnum)]
pub(in crate::cli) enum ProvenancePolicyArg {
    /// Record minimal execution provenance metadata.
    Minimal,
    /// Record complete reproducible build and environment provenance.
    Reproducible,
    /// Apply user-specified custom environment capture rules.
    Custom,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum ColorPolicyArg {
    /// Automatically infer color environment settings from the host.
    Auto,
    /// Force color environment variables to enabled state.
    Always,
    /// Force color environment variables to disabled state.
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
    pub(in crate::cli) fn policy(
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
