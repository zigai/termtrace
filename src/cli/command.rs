use std::{env, fmt::Write as _};

use clap::{CommandFactory as _, Parser, Subcommand, ValueEnum as _};

use crate::cli::{
    analysis::args::{
        AnalyzeArgs, AnalyzeSubcommand, DiagnoseFacadeArgs, InspectFacadeArgs, PlaybackArgs,
        PlaybackWorkerArgs, ShowArgs,
    },
    artifacts::args::{
        ArtifactArgs, ArtifactSubcommand, BugReportFacadeArgs, BundleArgs, ScreenshotArgs,
        VideoFacadeArgs,
    },
    control::args::{ActArgs, ControlArgs, ScreenArgs, SendArgs, StatusArgs, StopArgs, WaitArgs},
    integrity::args::{IntegrityArgs, IntegritySubcommand},
    output::{ResponseField, ResponseMode},
    owned::OwnedProcessArgs,
    recording::args::{GeometryProbeArgs, RecordArgs, RestartArgs, StartArgs},
    registry::args::{RegistryArgs, RegistrySubcommand},
    scenario::args::TestArgs,
    tmux::{TmuxArgs, TmuxSubcommand},
    tooling::args::{ClientCommand, DoctorArgs, ToolingArgs, ToolingSubcommand, VersionArgs},
};

const ROOT_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}{after-help}

Options:
{options}
";

const ROOT_COMMAND_GROUPS: &[(&str, &[&str])] = &[
    (
        "Sessions",
        &[
            "record", "start", "restart", "status", "stop", "session", "tmux",
        ],
    ),
    (
        "Interaction",
        &["screen", "show", "wait", "act", "send", "control"],
    ),
    ("Investigation", &["inspect", "diagnose", "analyze"]),
    (
        "Artifacts",
        &[
            "screenshot",
            "video",
            "bundle",
            "bug-report",
            "artifact",
            "integrity",
        ],
    ),
    ("Automation", &["test", "agent", "tooling"]),
    ("System", &["doctor", "version"]),
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
    Act(ActArgs),
    /// Send text, file, or standard-input bytes to a live child PTY.
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
    /// Create a manifested screenshot artifact.
    Screenshot(ScreenshotArgs),
    /// Create a sensitivity-labeled evidence bundle.
    Bundle(BundleArgs),
    /// Publish a bounded manifested debugging evidence export without raw byte streams.
    BugReport(BugReportFacadeArgs),
    /// Plan, preview, or export a manifested presentation video.
    Video(VideoFacadeArgs),
    /// Manage friendly session names and retention.
    Session(RegistryArgs),
    /// Verify, sign, and recover canonical evidence.
    Integrity(IntegrityArgs),
    /// Runtime, SDK, and shell integration tools.
    Tooling(ToolingArgs),
    /// Probe built-in and optional host capabilities.
    Doctor(DoctorArgs),
    /// Report build and runtime component versions.
    Version(VersionArgs),
    /// Serve the versioned language-neutral JSONL process protocol.
    Agent,

    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true)]
    Playback(PlaybackArgs),
    /// Internal persistent endpoint replay used as Ghostty's child process.
    #[command(hide = true)]
    PlaybackWorker(PlaybackWorkerArgs),
    /// Supervise one private native process group against parent process identity.
    #[command(hide = true)]
    OwnedProcess(OwnedProcessArgs),
    /// Internal live terminal geometry probe.
    #[command(hide = true)]
    GeometryProbe(GeometryProbeArgs),
    /// Return a compact representation of the latest presented screen.
    Screen(ScreenArgs),
    /// Show a truth-labeled live or completed terminal viewport.
    Show(ShowArgs),
    /// Wait for an explicit presented-screen, output-history, or idle predicate.
    Wait(WaitArgs),
}

pub(super) fn completion_command() -> clap::Command {
    return Cli::command();
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
    return ResponseMode::from_str(value, false).ok();
}

pub(super) fn command_contract(matches: &clap::ArgMatches) -> (String, bool) {
    let mut command_type = "termtrace";
    let mut current = matches;
    while let Some((name, subcommand)) = current.subcommand() {
        command_type = name;
        current = subcommand;
    }
    let mutating = matches!(
        command_type,
        "act"
            | "send"
            | "start"
            | "restart"
            | "stop"
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
        | Command::PlaybackWorker(_)
        | Command::OwnedProcess(_)
        | Command::GeometryProbe(_) => true,
        Command::Show(_) => true,
        Command::Artifact(arguments) => {
            matches!(arguments.command, ArtifactSubcommand::Capture(_))
        }
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
            ToolingSubcommand::Completions(_) => true,
            ToolingSubcommand::Client(arguments) => {
                !matches!(arguments.command, ClientCommand::Status)
            }
            ToolingSubcommand::Schemas(_) => false,
        },
        Command::Doctor(arguments) => arguments.json || !arguments.require.is_empty(),
        Command::Version(arguments) => arguments.json,
        Command::Session(arguments) => matches!(
            arguments.command,
            RegistrySubcommand::Resolve(_) | RegistrySubcommand::List(_)
        ),
        Command::Integrity(arguments) => matches!(
            arguments.command,
            IntegritySubcommand::Verify(_) | IntegritySubcommand::VerifyBundle(_)
        ),
        Command::Tmux(arguments) => matches!(arguments.command, TmuxSubcommand::Config(_)),
        _ => false,
    };
}
