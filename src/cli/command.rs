use crate::cli::{
    analysis::args::{AnalyzeArgs, InspectFacadeArgs, PlaybackArgs, PlaybackWorkerArgs},
    artifacts::args::{ArtifactArgs, BundleArgs, ScreenshotArgs, VideoFacadeArgs},
    control::args::{ScreenArgs, ScreenEncoding, SendArgs, StatusArgs, StopArgs, WaitArgs},
    integrity::args::IntegrityArgs,
    owned::OwnedProcessArgs,
    recording::args::{
        DurabilityModeArg, GeometryProbeArgs, ProvenancePolicyArg, RecordArgs, RecordingArgs,
        RestartArgs,
    },
    registry::args::RegistryArgs,
    scenario::args::TestArgs,
    tmux::TmuxArgs,
    tooling::args::{CompletionsArgs, DoctorArgs, SchemaArgs, VersionArgs},
};
use clap::{error::ErrorKind as ClapErrorKind, CommandFactory as _, Parser, Subcommand};
use std::env;
use termtrace_media::helper_protocol;
use termtrace_process::owned;
#[derive(Debug, Parser)]
#[command(
    name = "termtrace",
    version = concat!(env!("CARGO_PKG_VERSION"), " (build ", env!("TERMTRACE_BUILD_REVISION"), ")"),
    about = "Record native terminal pixels and every semantic terminal state",

    disable_help_flag = true
) ]
pub(super) struct Cli {
    /// Print help.
    #[arg(long, global = true, action = clap::ArgAction::Help)]
    help: Option<bool>,

    /// Suppress standard output and communicate only via exit codes.
    #[arg(short = 'q', long, global = true)]
    pub(super) quiet: bool,

    /// Diagnostic log level threshold (error, warn, info, debug, trace).
    #[arg(long, global = true, env = "TERMTRACE_LOG", default_value = "warn")]
    pub(super) log: tracing::Level,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
#[expect(clippy::large_enum_variant, reason = "Top-level command dispatch enum")]
pub(super) enum Command {
    /// Record a terminal command (use -d/--detach for a background session).
    Record(RecordArgs),
    /// View the live or historical presented terminal viewport.
    Screen(ScreenArgs),
    /// Send terminal input (keys, text, clicks, resize) and await screen state.
    Send(SendArgs),
    /// Wait for screen text, output bytes, or idle stability.
    Wait(WaitArgs),
    /// Show live session status or list registered sessions (--gc to prune).
    Status(StatusArgs),
    /// Request graceful termination of a running background session.
    Stop(StopArgs),
    /// Atomically stop and restart a background session.
    Restart(RestartArgs),
    /// Inspect session forensics (default: triage last action).
    Analyze(AnalyzeArgs),
    /// Capture a rendered PNG/SVG screenshot of a session.
    Screenshot(ScreenshotArgs),
    /// Export a presentation MP4 video from a session.
    Video(VideoFacadeArgs),
    /// Create an evidence bundle (supports --profile debug and --screenshot).
    Bundle(BundleArgs),
    /// Execute strict evidence-native terminal scenario tests.
    Test(TestArgs),
    /// Record managed tmux panes for their full lifetime.
    Tmux(TmuxArgs),
    /// Generate a shell autocompletion script from the current command graph.
    #[command(name = "completion")]
    Completion(CompletionsArgs),
    /// Diagnose the installation, dependencies, state, and capabilities.
    Doctor(DoctorArgs),
    /// Serve the session workflow as Model Context Protocol tools over stdio.
    Mcp,

    /// Generate JSON Schemas for public events, controls, and artifacts.
    #[command(name = "schema", hide = true)]
    Schema(SchemaArgs),
    /// Report build and runtime component versions.
    #[command(hide = true)]
    Version(VersionArgs),

    /// Internal timed event playback used as Ghostty's child process.
    #[command(hide = true, name = helper_protocol::playback::SUBCOMMAND)]
    Playback(PlaybackArgs),
    /// Internal persistent endpoint replay used as Ghostty's child process.
    #[command(hide = true, name = helper_protocol::playback_worker::SUBCOMMAND)]
    PlaybackWorker(PlaybackWorkerArgs),
    /// Supervise one private native process group against parent process identity.
    #[command(hide = true, name = owned::SUBCOMMAND)]
    OwnedProcess(OwnedProcessArgs),
    /// Internal live terminal geometry probe.
    #[command(hide = true, name = helper_protocol::geometry_probe::SUBCOMMAND)]
    GeometryProbe(GeometryProbeArgs),
    /// Internal artifact operations.
    #[command(hide = true)]
    Artifact(ArtifactArgs),
    /// Internal session registry operations.
    #[command(hide = true)]
    Session(RegistryArgs),
    /// Internal integrity operations.
    #[command(hide = true)]
    Integrity(IntegrityArgs),
    /// Internal last-action shortcut.
    #[command(name = "last-action", hide = true)]
    LastAction(InspectFacadeArgs),
}

pub(super) fn completion() -> clap::Command {
    return Cli::command();
}

fn bound_help_width(command: clap::Command) -> clap::Command {
    return command
        .max_term_width(100)
        .mut_subcommands(bound_help_width);
}

pub(super) fn should_disable_color() -> bool {
    return env::var_os("NO_COLOR").is_some()
        || env::var_os("NOCOLOR").is_some()
        || env::var("TERM").is_ok_and(|term| return term == "dumb");
}

pub(super) fn root() -> clap::Command {
    let mut command = bound_help_width(Cli::command());
    if should_disable_color() {
        command = command.color(clap::ColorChoice::Never);
    }

    return command;
}

impl Cli {
    pub(super) fn is_quiet(&self) -> bool {
        return self.quiet;
    }

    pub(super) fn validate(&self) -> Result<(), clap::Error> {
        let mut cmd = root();
        match &self.command {
            Command::Record(record_args) => {
                validate_recording_args(&mut cmd, &record_args.recording)?;
            }

            Command::Restart(restart_args) => {
                if !restart_args.provenance_environment.is_empty()
                    && !matches!(restart_args.provenance, Some(ProvenancePolicyArg::Custom))
                {
                    return Err(cmd.error(
                        ClapErrorKind::ArgumentConflict,
                        "--provenance-env requires --provenance custom\n\nhelp: specify `--provenance custom` when supplying custom environment rules",
                    ));
                }

                if (restart_args.durability_max_interval_ms.is_some()
                    || restart_args.durability_max_uncommitted_bytes.is_some())
                    && !matches!(restart_args.durability, Some(DurabilityModeArg::Bounded))
                {
                    return Err(cmd.error(
                        ClapErrorKind::ArgumentConflict,
                        "--durability-max-interval-ms and --durability-max-uncommitted-bytes require --durability bounded\n\nhelp: specify `--durability bounded` when supplying durability thresholds",
                    ));
                }
            }
            Command::Screen(screen_args)
                if screen_args.screen == ScreenEncoding::Diff && screen_args.since.is_none() =>
            {
                return Err(cmd.error(
                    ClapErrorKind::MissingRequiredArgument,
                    "--screen diff requires --since STATE_HASH\n\nhelp: specify `--since <STATE_HASH>` to compare the latest presented screen against",
                ));
            }

            _ => {}
        }

        return Ok(());
    }
}

fn validate_recording_args(
    cmd: &mut clap::Command,
    recording: &RecordingArgs,
) -> Result<(), clap::Error> {
    if !recording.provenance_environment.is_empty()
        && !matches!(recording.provenance, ProvenancePolicyArg::Custom)
    {
        return Err(cmd.error(
            ClapErrorKind::ArgumentConflict,
            "--provenance-env requires --provenance custom\n\nhelp: specify `--provenance custom` when supplying custom environment rules",
        ));
    }

    if (recording.durability_max_interval_ms.is_some()
        || recording.durability_max_uncommitted_bytes.is_some())
        && !matches!(recording.durability, DurabilityModeArg::Bounded)
    {
        return Err(cmd.error(
            ClapErrorKind::ArgumentConflict,
            "--durability-max-interval-ms and --durability-max-uncommitted-bytes require --durability bounded\n\nhelp: specify `--durability bounded` when supplying durability thresholds",
        ));
    }
    return Ok(());
}

pub(super) fn requested_quiet_mode() -> bool {
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let Some(s) = argument.to_str() else {
            continue;
        };
        if s == "--" {
            break;
        }

        if s == "--quiet" {
            return true;
        }

        if s == "--log" || s == "-o" || s == "--out" || s == "-L" || s == "-S" {
            arguments.next();
            continue;
        }

        if s.starts_with("--") {
            continue;
        }

        if let Some(chars) = s.strip_prefix('-') {
            if chars.is_empty() {
                continue;
            }

            for (idx, ch) in chars.char_indices() {
                match ch {
                    'q' => return true,
                    'o' | 'L' | 'S' => {
                        if idx + ch.len_utf8() == chars.len() {
                            arguments.next();
                        }

                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    return false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_helpers_preserve_subcommand_and_flag_contracts() {
        let command = Cli::command();

        for (name, mut expected_flags) in [
            (
                "playback",
                vec![
                    "session",
                    "timing-file",
                    "error-file",
                    "geometry-file",
                    "ready-file",
                    "start-delay-ms",
                    "final-hold-ms",
                    "from-ns",
                    "to-ns",
                    "endpoint-event-seq",
                    "endpoint-event-byte-offset",
                    "instant",
                ],
            ),
            (
                "playback-worker",
                vec!["session", "socket", "geometry-file", "error-file"],
            ),
            ("geometry-probe", vec!["out"]),
            ("owned-process", vec!["parent-pid", "parent-start-ticks"]),
        ] {
            let helper = command
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("missing helper subcommand {name}"));
            assert!(helper.is_hide_set(), "{name} must stay hidden");
            let mut flags = helper
                .get_arguments()
                .filter(|argument| return !argument.is_global_set())
                .filter_map(clap::Arg::get_long)
                .collect::<Vec<_>>();
            flags.sort_unstable();
            expected_flags.sort_unstable();
            assert_eq!(flags, expected_flags, "{name}");
        }
    }
}
