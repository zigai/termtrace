//! Command-line entry point for Termtrace.

use std::{
    collections::VecDeque,
    env,
    ffi::{OsStr, OsString},
    fmt::{Display, Write as _},
    fs::{File, OpenOptions, Permissions},
    io::{self, BufReader, Read as _, Write as _},
    mem::take,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, ExitStatus, Stdio},
    result::Result as StdResult,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use clap::{
    ArgGroup, Args, CommandFactory as _, FromArgMatches as _, Parser, Subcommand, ValueEnum,
};
use clap_complete::{Shell, generate};
use miette::{IntoDiagnostic as _, Result};
use termtrace::{
    Error,
    analysis::{
        DiagnoseReport, DiagnosticCheck, ExplainOutputPage, ExplainReport, diagnose, explain,
    },
    artifact::write_secure,
    bundle::{
        BundleProfile, BundleVerificationPolicy, archive as archive_bundle,
        create as create_bundle, plan as plan_bundle, verify as verify_bundle,
        verify_archive as verify_bundle_archive,
    },
    capability::{
        CAPABILITY_IDS, CapabilityReport, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY,
        NATIVE_VIDEO_CAPABILITY, probe_capabilities, require_capability,
    },
    control::{
        ActorBoundaries, CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlRequest,
        ControlResult, InputSequenceAtom, PointerOperation, ProcessSignal, ScreenMode,
        ScreenPredicate, ScreenRegion, StableCaptureQuality, StateHashComparison, request,
        send_request, send_request_response,
    },
    diff::{SemanticDiff, diff_states_with_semantic_states, inspect_point},
    driver::serve as serve_driver,
    environment::{
        ColorPolicy, EnvironmentBase, EnvironmentSpec, parse_environment_spec,
        read_environment_spec,
    },
    event::{ByteString, EVENT_LOG_VERSION, Event, EventProjection, InputKind, event_type},
    event_log::{DurabilityMode, DurabilityPolicy, EventLog},
    geometry::{inherited_geometry, run_probe},
    ghostty_vt::{
        CursorStyle, Format as GhosttyFormat, KeyModifiers, MouseButton, NamedKey, RowState,
        SemanticState,
    },
    inspection::{
        inspect_session, state_text, wait_for_completion, wait_for_output, wait_for_screen,
    },
    interop::export_asciicast,
    observation::{
        CaptureQuality, Observation, ObservationSourceKind, StatePoint, TruthLayer,
        format_ansi_stream, observe_ansi_stream, observe_one_off_session,
        observe_session_with_options, observe_validated_prefix, prune_replay_cache,
        replay_cache_status, semantic_state_from_ansi, semantic_state_with_hash_for_session,
    },
    output_projection::{ControlStripper, OutputProjectionQuality, OutputProjectionReport},
    pagination::OutputPage,
    playback::{PlaybackOptions, playback},
    presentation::{
        EditPlan, PresentationSource, PresentationSourceRequest, PreviewOptions,
        export as export_presentation, markers as list_markers,
        plan_with_source as plan_presentation, preview as preview_presentation,
    },
    profile::{
        ResolvedGhosttyProfile, resolve_ghostty_profile, resolved_terminal_colors,
        restore_ghostty_profile, terminal_colors_for_profile,
    },
    provenance::{ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind},
    receipt::create_ssh_receipt,
    record::{RecordOptions, RecordOutcome, record},
    replay::{
        GHOSTTY_REVISION, IndexOptions, STATE_FORMAT_VERSION, StateSelection, StateSelector,
        format_state, index_session_with_options,
    },
    reviewer::{
        ReviewerCapabilities, export_state as export_review_state, run as run_reviewer,
        search as search_review, summarize as summarize_review,
    },
    save::{SaveFormat, save, save_screenshot_batch},
    scenario::{
        Scenario, ScenarioAttemptReport, ScenarioCaseReport, ScenarioStatus, ScenarioSuiteReport,
        ScenarioSuiteStatus, run as run_scenario,
    },
    schema::{generate as generate_schemas, write_all as write_schemas},
    semantic_image::{
        SEMANTIC_PNG_RENDERER, SEMANTIC_SVG_RENDERER, render_diff_png, render_diff_svg, render_png,
        render_svg,
    },
    session::{
        CaptureTarget, GHOSTTY_REPLAY_PROFILE_NAME, GeometryProvenance, GeometrySource,
        GhosttyVisualOverrides, ImportProvenance, ResourceLimits, SessionGeometry, SessionLineage,
        SessionManifest, VerificationLevel, manifest_hash, read_manifest, validate_completed_log,
        verify_session_with_trust,
    },
    session_registry::{DetachedSessionPaths, RegistryEntry, SessionRegistry},
    timeline::{
        TimelineEntry, TimelineItem, build_timeline, parse_time_offset_ns, resolve_source_selection,
    },
    visual::{
        CaptureKind, CaptureOptions, GeometrySolveReport, capture, probe_ghostty_geometry,
        solve_ghostty_geometry,
    },
};
use tracing_subscriber::{
    EnvFilter, fmt, layer::SubscriberExt, registry as tracing_registry, util::SubscriberInitExt,
};

mod args_analysis;
mod args_artifacts;
mod args_control;
mod args_integrity;
mod args_recording;
mod args_registry;
mod args_scenario;
mod args_tooling;

use args_analysis::*;
use args_artifacts::*;
use args_control::*;
use args_integrity::*;
use args_recording::*;
use args_registry::*;
use args_scenario::*;
use args_tooling::*;

mod analysis_replay;
mod analysis_reports;
mod analysis_show;
mod analysis_streams;
mod artifacts;
mod control;
mod dispatch;
mod integrity;
mod output;
mod parsers;
mod recording;
mod registry;
mod scenario;
mod session_helpers;
mod tooling;

use analysis_replay::*;
use analysis_reports::*;
use analysis_show::*;
use analysis_streams::*;
use artifacts::*;
use control::*;
use integrity::*;
use output::*;
use parsers::*;
use recording::*;
use registry::*;
use scenario::*;
use session_helpers::*;
use tooling::*;

const ROOT_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}{after-help}

Options:
{options}
";

const ROOT_COMMAND_GROUPS: &[(&str, &[&str])] = &[
    ("Recording", &["record", "test", "start", "restart", "stop"]),
    (
        "Live workflow",
        &["status", "send", "wait", "wait-idle", "show"],
    ),
    (
        "Command groups",
        &[
            "control",
            "analyze",
            "artifact",
            "registry",
            "integrity",
            "tooling",
        ],
    ),
];

#[derive(Debug, Parser)]
#[command(
    name = "termtrace",
    version,
    about = "Record native terminal pixels and every semantic terminal state"
)]
struct Cli {
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    log: tracing::Level,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a command in a PTY and preserve its byte-exact event stream.
    Record(RecordArgs),
    /// Execute one strict evidence-native terminal scenario.
    Test(TestArgs),
    /// Start a detached recording and publish a name after readiness.
    Start(StartArgs),
    /// Stop a named session and atomically publish an immutable successor.
    Restart(RestartArgs),
    /// Print machine-readable live session status.
    Status(StatusArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Request graceful termination of a live session child.
    Stop(StopArgs),
    /// Live session control commands.
    Control(ControlArgs),
    /// Replay, inspect, compare, and diagnose terminal evidence.
    Analyze(AnalyzeArgs),
    /// Render, capture, and publish terminal artifacts.
    Artifact(ArtifactArgs),
    /// Register and manage friendly session names.
    Registry(RegistryArgs),
    /// Verify, sign, and recover canonical evidence.
    Integrity(IntegrityArgs),
    /// Runtime, SDK, and shell integration tools.
    Tooling(ToolingArgs),

    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true)]
    Playback(PlaybackArgs),
    /// Internal live terminal geometry probe.
    #[command(hide = true)]
    GeometryProbe(GeometryProbeArgs),
    /// Show a truth-labeled live or completed terminal viewport.
    Show(ShowArgs),
    /// Wait for exact UTF-8 text in canonical PTY output.
    Wait(WaitArgs),
    /// Atomically wait for a stable presented screen and return that exact snapshot.
    WaitIdle(WaitIdleArgs),
}

/// Completion generation uses the public command graph and omits internal commands.
#[derive(Debug, Parser)]
#[command(
    name = "termtrace",
    version,
    about = "Record native terminal pixels and every semantic terminal state"
)]
struct CompletionCli {
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    log: tracing::Level,

    #[command(subcommand)]
    command: CompletionCommand,
}

#[derive(Debug, Subcommand)]
enum CompletionCommand {
    /// Run a command in a PTY and preserve its byte-exact event stream.
    Record(RecordArgs),
    /// Execute one strict evidence-native terminal scenario.
    Test(TestArgs),
    /// Start a detached recording and publish a name after readiness.
    Start(StartArgs),
    /// Stop a named session and atomically publish an immutable successor.
    Restart(RestartArgs),
    /// Request graceful termination of a live session child.
    Stop(StopArgs),
    /// Print machine-readable live session status.
    Status(StatusArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Wait for exact UTF-8 text in canonical PTY output.
    Wait(WaitArgs),
    /// Atomically wait for a stable presented screen and return that exact snapshot.
    WaitIdle(WaitIdleArgs),
    /// Show a truth-labeled live or completed terminal viewport.
    Show(ShowArgs),
    /// Live session control commands.
    Control(ControlArgs),
    /// Replay, inspect, compare, and diagnose terminal evidence.
    Analyze(AnalyzeArgs),
    /// Render, capture, and publish terminal artifacts.
    Artifact(ArtifactArgs),
    /// Register and manage friendly session names.
    Registry(RegistryArgs),
    /// Verify, sign, and recover canonical evidence.
    Integrity(IntegrityArgs),
    /// Runtime, SDK, and shell integration tools.
    Tooling(ToolingArgs),
}

fn init_diagnostics(level: tracing::Level) {
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    tracing_registry()
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .with(fmt::layer().with_target(false).with_writer(io::stderr))
        .init();
}

fn bound_help_width(command: clap::Command) -> clap::Command {
    return command
        .max_term_width(100)
        .mut_subcommands(bound_help_width);
}

fn root_command() -> clap::Command {
    let command = bound_help_width(Cli::command());
    let visible_commands = command
        .get_subcommands()
        .filter(|subcommand| return !subcommand.is_hide_set())
        .map(|subcommand| {
            return (
                subcommand.get_name().to_owned(),
                subcommand
                    .get_about()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            );
        })
        .collect::<Vec<_>>();
    let grouped_names = ROOT_COMMAND_GROUPS
        .iter()
        .flat_map(|(_, names)| return names.iter().copied())
        .collect::<Vec<_>>();

    for (name, _) in &visible_commands {
        assert!(
            grouped_names.contains(&name.as_str()),
            "visible command `{name}` is missing from ROOT_COMMAND_GROUPS"
        );
    }
    for name in &grouped_names {
        assert!(
            visible_commands
                .iter()
                .any(|(visible, _)| return visible == name),
            "ROOT_COMMAND_GROUPS contains unknown or hidden command `{name}`"
        );
    }

    let command_width = grouped_names
        .iter()
        .map(|name| return name.len())
        .max()
        .unwrap_or_default();
    let mut grouped_help = String::from("Commands:\n");
    for (heading, names) in ROOT_COMMAND_GROUPS {
        let _ = writeln!(grouped_help, "  {heading}:");
        for name in *names {
            let Some(about) = visible_commands.iter().find_map(|(visible, about)| {
                return (visible == name).then_some(about.as_str());
            }) else {
                continue;
            };
            let _ = writeln!(grouped_help, "    {name:<command_width$}  {about}");
        }
        grouped_help.push('\n');
    }
    grouped_help.push_str("Run `termtrace <command> --help` for more details.");

    return command
        .help_template(ROOT_HELP_TEMPLATE)
        .after_help(grouped_help);
}

pub(crate) fn run() -> Result<ExitCode> {
    let matches = root_command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    init_diagnostics(cli.log);
    return dispatch::dispatch(cli.command);
}
