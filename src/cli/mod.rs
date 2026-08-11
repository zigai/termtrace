//! Command-line entry point for Termtrace.

use std::{
    collections::VecDeque,
    env,
    ffi::{OsStr, OsString},
    fmt::{Display, Write as _},
    fs::{File, OpenOptions, Permissions},
    io::{self, BufReader, IsTerminal as _, Read as _, Write as _},
    mem::take,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{OpenOptionsExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, ExitStatus, Stdio},
    result::Result as StdResult,
    sync::{Arc, Mutex, OnceLock, mpsc},
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
        ActionPostcondition, ActionPostconditionResult, ActorBoundaries, CONTROL_PROTOCOL_VERSION,
        ControlCommand, ControlData, ControlErrorDetails, ControlRequest, ControlResult,
        InputSequenceAtom, PointerOperation, ProcessSignal, ScreenMode, ScreenPredicate,
        ScreenRegion, StableCaptureQuality, StateHashComparison, control_endpoint_is_reachable,
        request, send_request, send_request_response,
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
        EditClip, EditPlan, EditSelector, PixelTransform, PresentationDelivery, PresentationSource,
        PresentationSourceRequest, PreviewOptions, RationalSpeed, VideoRenderConfig,
        export as export_presentation, markers as list_markers,
        plan_with_source as plan_presentation, preview as preview_presentation,
    },
    profile::{
        ResolvedGhosttyProfile, resolve_ghostty_profile, resolved_terminal_colors,
        restore_ghostty_profile, terminal_colors_for_profile,
    },
    provenance::{ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind},
    receipt::create_ssh_receipt,
    record::{RecordOptions, RecordOutcome, record, record_with_output},
    replay::{
        GHOSTTY_REVISION, IndexOptions, STATE_FORMAT_VERSION, StateSelection, StateSelector,
        format_state, index_session_with_options, semantic_state_hash,
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
        GhosttyVisualOverrides, ImportProvenance, ResourceLimits, SessionCompletion,
        SessionGeometry, SessionLineage, SessionManifest, TMUX_PANE_PROVENANCE_VERSION,
        TmuxPaneLaunchKind, TmuxPaneProvenance, VerificationLevel, manifest_hash, read_manifest,
        validate_completed_log, verify_session_with_trust,
    },
    session_registry::{
        AliasReservation, DetachedSessionPaths, RegistryEntry, RegistryLifecycle, SessionRegistry,
    },
    task_facade::{analyze_last_action, create_bug_report},
    timeline::{
        SourceRange, TimelineEntry, TimelineItem, build_timeline, build_timeline_page,
        parse_time_offset_ns, resolve_source_selection,
    },
    tmux_bundle::create_tmux_bundle,
    visual::{
        CaptureKind, CaptureOptions, GeometrySolveReport, capture, probe_ghostty_geometry,
        solve_ghostty_geometry,
    },
};
use tracing_subscriber::{
    EnvFilter, fmt, layer::SubscriberExt, registry as tracing_registry, util::SubscriberInitExt,
};

mod analysis;
mod args;
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
mod tmux;
mod tooling;

use analysis::*;
use args::*;
use artifacts::*;
use control::*;
use integrity::*;
use output::*;
use parsers::*;
use recording::*;
use registry::*;
use scenario::*;
use session_helpers::*;
use tmux::*;
use tooling::*;

const ROOT_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}{after-help}

Options:
{options}
";

const ROOT_COMMAND_GROUPS: &[(&str, &[&str])] = &[
    (
        "Recording",
        &["record", "test", "start", "restart", "stop", "tmux"],
    ),
    (
        "Live workflow",
        &[
            "status",
            "act",
            "send",
            "screen",
            "show",
            "wait",
            "wait-idle",
        ],
    ),
    ("Agent discovery", &["capabilities", "agent"]),
    ("Debugging", &["inspect", "diagnose"]),
    ("Media", &["screenshot", "video"]),
    ("Handoff", &["bundle", "bug-report"]),
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

    /// Select concise human, strict agent, forensic full, or exit-only output.
    #[arg(
        long,
        global = true,
        env = "TERMTRACE_RESPONSE",
        default_value = "auto",
        value_name = "PROFILE"
    )]
    response: ResponseMode,

    /// Include advanced compact-response fields.
    #[arg(long, global = true, value_delimiter = ',', value_name = "FIELD")]
    fields: Vec<ResponseField>,

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
    /// Apply terminal input and wait for an optional presented-state postcondition.
    #[command(visible_alias = "input")]
    Act(ActArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Request graceful termination of a live session child.
    Stop(StopArgs),
    /// Record managed tmux panes for their full lifetime.
    Tmux(TmuxArgs),
    /// Live session control commands.
    Control(ControlArgs),
    /// Replay, inspect, compare, and diagnose terminal evidence.
    Analyze(AnalyzeArgs),
    /// Render, capture, and publish terminal artifacts.
    Artifact(ArtifactArgs),
    /// Summarize the most recent canonical input action.
    Inspect(InspectFacadeArgs),
    /// Run diagnostics bounded to the most recent canonical input action.
    Diagnose(DiagnoseFacadeArgs),
    /// Render the last presented semantic screen directly to a PNG file.
    Screenshot(ScreenshotFacadeArgs),
    /// Publish the default bounded manifested session handoff.
    Bundle(BundleFacadeArgs),
    /// Publish a bounded manifested debugging handoff without raw byte streams.
    BugReport(BugReportFacadeArgs),
    /// Export a manifested presentation video without the advanced command tree.
    Video(VideoFacadeArgs),
    /// Register and manage friendly session names.
    Registry(RegistryArgs),
    /// Verify, sign, and recover canonical evidence.
    Integrity(IntegrityArgs),
    /// Runtime, SDK, and shell integration tools.
    Tooling(ToolingArgs),
    /// Probe built-in and optional host capabilities.
    Capabilities(DoctorArgs),
    /// Serve the versioned language-neutral JSONL process protocol.
    #[command(visible_alias = "rpc")]
    Agent,

    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true)]
    Playback(PlaybackArgs),
    /// Internal live terminal geometry probe.
    #[command(hide = true)]
    GeometryProbe(GeometryProbeArgs),
    /// Return a compact representation of the latest presented screen.
    Screen(ScreenArgs),
    /// Show a truth-labeled terminal viewport using the advanced format surface.
    Show(ShowArgs),
    /// Wait for an explicit presented-screen, output-history, or idle predicate.
    #[command(visible_alias = "wait-screen")]
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

    /// Select concise human, strict agent, forensic full, or exit-only output.
    #[arg(
        long,
        global = true,
        env = "TERMTRACE_RESPONSE",
        default_value = "auto",
        value_name = "PROFILE"
    )]
    response: ResponseMode,

    /// Include advanced compact-response fields.
    #[arg(long, global = true, value_delimiter = ',', value_name = "FIELD")]
    fields: Vec<ResponseField>,

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
    /// Record managed tmux panes for their full lifetime.
    Tmux(TmuxArgs),
    /// Print machine-readable live session status.
    Status(StatusArgs),
    /// Apply terminal input and wait for an optional presented-state postcondition.
    #[command(visible_alias = "input")]
    Act(ActArgs),
    /// Send text or file bytes to a live child PTY.
    Send(SendArgs),
    /// Wait for an explicit presented-screen, output-history, or idle predicate.
    #[command(visible_alias = "wait-screen")]
    Wait(WaitArgs),
    /// Atomically wait for a stable presented screen and return that exact snapshot.
    WaitIdle(WaitIdleArgs),
    /// Show a truth-labeled live or completed terminal viewport.
    Show(ShowArgs),
    /// Return a compact representation of the latest presented screen.
    Screen(ScreenArgs),
    /// Live session control commands.
    Control(ControlArgs),
    /// Replay, inspect, compare, and diagnose terminal evidence.
    Analyze(AnalyzeArgs),
    /// Render, capture, and publish terminal artifacts.
    Artifact(ArtifactArgs),
    /// Summarize the most recent canonical input action.
    Inspect(InspectFacadeArgs),
    /// Run diagnostics bounded to the most recent canonical input action.
    Diagnose(DiagnoseFacadeArgs),
    /// Render the last presented semantic screen directly to a PNG file.
    Screenshot(ScreenshotFacadeArgs),
    /// Publish the default bounded manifested session handoff.
    Bundle(BundleFacadeArgs),
    /// Publish a bounded manifested debugging handoff without raw byte streams.
    BugReport(BugReportFacadeArgs),
    /// Export a manifested presentation video without the advanced command tree.
    Video(VideoFacadeArgs),
    /// Register and manage friendly session names.
    Registry(RegistryArgs),
    /// Verify, sign, and recover canonical evidence.
    Integrity(IntegrityArgs),
    /// Runtime, SDK, and shell integration tools.
    Tooling(ToolingArgs),
    /// Probe built-in and optional host capabilities.
    Capabilities(DoctorArgs),
    /// Serve the versioned language-neutral JSONL process protocol.
    #[command(visible_alias = "rpc")]
    Agent,
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

    let mut grouped_help = String::from("Commands:\n");
    for (heading, names) in ROOT_COMMAND_GROUPS {
        let command_width = names
            .iter()
            .map(|name| return name.len())
            .max()
            .unwrap_or_default()
            .max(9);
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
    grouped_help.push_str(
        "A presented screen is a state snapshot; canonical output is the byte history.\n\
         \n\
         Agent workflow:\n\
           termtrace --response agent start NAME --until-screen TEXT -- COMMAND\n\
           termtrace --response agent act SESSION INPUT... --until-idle DURATION\n\
           termtrace --response agent screen SESSION --since STATE\n\
         Use `termtrace agent` after 2–3 actions. Treat applied, retry, and session_state as\n\
         authoritative. Use root help and direct leaf --help; request fields instead of recursively\n\
         traversing help or shell-filtering responses.\n\
         \n\
         Run `termtrace <command> --help` for more details.",
    );

    return command
        .help_template(ROOT_HELP_TEMPLATE)
        .after_help(grouped_help);
}

fn requested_response_mode() -> ResponseMode {
    let mut mode = env::var("TERMTRACE_RESPONSE")
        .ok()
        .and_then(|value| return response_mode_name(&value))
        .unwrap_or(ResponseMode::Auto);
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if let Some(value) = argument.strip_prefix("--response=") {
            if let Some(selected) = response_mode_name(value) {
                mode = selected;
            }
            continue;
        }
        if argument != "--response" {
            continue;
        }
        if let Some(value) = arguments
            .next()
            .and_then(|value| return value.into_string().ok())
            && let Some(selected) = response_mode_name(&value)
        {
            mode = selected;
        }
    }
    return mode;
}

fn response_mode_name(value: &str) -> Option<ResponseMode> {
    return match value {
        "auto" => Some(ResponseMode::Auto),
        "human" => Some(ResponseMode::Human),
        "agent" => Some(ResponseMode::Agent),
        "full" => Some(ResponseMode::Full),
        "quiet" => Some(ResponseMode::Quiet),
        _ => None,
    };
}

fn command_contract(matches: &clap::ArgMatches) -> (String, bool) {
    let mut command_type = "termtrace";
    let mut current = matches;
    while let Some((name, subcommand)) = current.subcommand() {
        command_type = name;
        current = subcommand;
    }
    if matches!(command_type, "wait-idle" | "wait-screen") {
        command_type = "wait";
    }
    let mutating = matches!(
        command_type,
        "act"
            | "send"
            | "start"
            | "restart"
            | "stop"
            | "input"
            | "key"
            | "paste"
            | "pointer"
            | "resize"
            | "signal"
            | "mark"
            | "enable"
            | "disable"
            | "set-scope"
            | "remove"
    );
    return (command_type.to_owned(), mutating);
}

fn preserves_requested_output(command: &Command) -> bool {
    return match command {
        Command::Record(_)
        | Command::Video(_)
        | Command::Inspect(_)
        | Command::Diagnose(_)
        | Command::Screenshot(_)
        | Command::Bundle(_)
        | Command::BugReport(_)
        | Command::Agent
        | Command::Playback(_)
        | Command::GeometryProbe(_) => true,
        Command::Show(_) => true,
        Command::Artifact(arguments) => matches!(
            arguments.command,
            ArtifactSubcommand::Capture(_)
                | ArtifactSubcommand::Screenshot(_)
                | ArtifactSubcommand::Video(_)
        ),
        Command::Analyze(arguments) => matches!(
            arguments.command,
            AnalyzeSubcommand::Inspect(_)
                | AnalyzeSubcommand::Timeline(_)
                | AnalyzeSubcommand::Diff(_)
                | AnalyzeSubcommand::Explain(_)
                | AnalyzeSubcommand::Diagnose(_)
                | AnalyzeSubcommand::Output(_)
                | AnalyzeSubcommand::Events(_)
        ),
        Command::Tooling(arguments) => match &arguments.command {
            ToolingSubcommand::Driver | ToolingSubcommand::Completions(_) => true,
            ToolingSubcommand::Client(arguments) => {
                !matches!(arguments.command, ClientCommand::Status)
            }
            ToolingSubcommand::Doctor(arguments) => arguments.json || !arguments.require.is_empty(),
            ToolingSubcommand::Version(arguments) => arguments.json,
            ToolingSubcommand::Schemas(_) => false,
        },
        Command::Capabilities(arguments) => arguments.json || !arguments.require.is_empty(),
        Command::Registry(arguments) => matches!(
            arguments.command,
            RegistrySubcommand::Resolve(_) | RegistrySubcommand::List(_)
        ),
        Command::Integrity(arguments) => {
            matches!(arguments.command, IntegritySubcommand::Verify(_))
        }
        Command::Tmux(arguments) => matches!(arguments.command, TmuxSubcommand::Config(_)),
        _ => false,
    };
}

pub(crate) fn run() -> ExitCode {
    let requested_mode = requested_response_mode();
    let matches = match root_command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => return write_clap_error(error, requested_mode),
    };
    let (command_type, mutating) = command_contract(&matches);
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => return write_clap_error(error, requested_mode),
    };
    let response_mode =
        if cli.response == ResponseMode::Auto && preserves_requested_output(&cli.command) {
            ResponseMode::Full
        } else {
            cli.response.resolve()
        };
    configure_response(response_mode, cli.fields, command_type, mutating);
    if response_mode != ResponseMode::Quiet {
        init_diagnostics(cli.log);
    }
    return finish_response(dispatch::dispatch(cli.command));
}
