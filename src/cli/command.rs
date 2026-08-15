use std::{env, fmt::Write as _};

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::cli::{
    analysis::args::{
        AnalyzeArgs, AnalyzeSubcommand, DiagnoseFacadeArgs, InspectFacadeArgs, PlaybackArgs,
        ShowArgs,
    },
    artifacts::args::{
        ArtifactArgs, ArtifactSubcommand, BugReportFacadeArgs, BundleFacadeArgs,
        ScreenshotFacadeArgs, VideoFacadeArgs,
    },
    control::args::{
        ActArgs, ControlArgs, ScreenArgs, SendArgs, StatusArgs, StopArgs, WaitArgs, WaitIdleArgs,
    },
    integrity::args::{IntegrityArgs, IntegritySubcommand},
    output::{ResponseField, ResponseMode},
    recording::args::{GeometryProbeArgs, RecordArgs, RestartArgs, StartArgs},
    registry::args::{RegistryArgs, RegistrySubcommand},
    scenario::args::TestArgs,
    tmux::{TmuxArgs, TmuxSubcommand},
    tooling::args::{ClientCommand, DoctorArgs, ToolingArgs, ToolingSubcommand},
};

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
    ("Evidence export", &["bundle", "bug-report"]),
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
pub(super) struct Cli {
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    pub(super) log: tracing::Level,

    /// Select concise human, strict agent, forensic full, or exit-only output.
    #[arg(
        long,
        global = true,
        env = "TERMTRACE_RESPONSE",
        default_value = "auto",
        value_name = "PROFILE"
    )]
    pub(super) response: ResponseMode,

    /// Include advanced compact-response fields.
    #[arg(long, global = true, value_delimiter = ',', value_name = "FIELD")]
    pub(super) fields: Vec<ResponseField>,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
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
    /// Publish the default bounded manifested session evidence export.
    Bundle(BundleFacadeArgs),
    /// Publish a bounded manifested debugging evidence export without raw byte streams.
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
pub(super) struct CompletionCli {
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    pub(super) log: tracing::Level,

    /// Select concise human, strict agent, forensic full, or exit-only output.
    #[arg(
        long,
        global = true,
        env = "TERMTRACE_RESPONSE",
        default_value = "auto",
        value_name = "PROFILE"
    )]
    pub(super) response: ResponseMode,

    /// Include advanced compact-response fields.
    #[arg(long, global = true, value_delimiter = ',', value_name = "FIELD")]
    pub(super) fields: Vec<ResponseField>,

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
    /// Publish the default bounded manifested session evidence export.
    Bundle(BundleFacadeArgs),
    /// Publish a bounded manifested debugging evidence export without raw byte streams.
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
fn bound_help_width(command: clap::Command) -> clap::Command {
    return command
        .max_term_width(100)
        .mut_subcommands(bound_help_width);
}

pub(super) fn root_command() -> clap::Command {
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

pub(super) fn requested_response_mode() -> ResponseMode {
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

pub(super) fn command_contract(matches: &clap::ArgMatches) -> (String, bool) {
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

pub(super) fn preserves_requested_output(command: &Command) -> bool {
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
