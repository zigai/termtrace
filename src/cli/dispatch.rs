use crate::cli::command::Command;
use crate::cli::{
    analysis::{
        dispatch as dispatch_analyze, run_last_action_facade, run_playback, run_playback_worker,
    },
    artifacts::{self, run_bundle, run_screenshot, run_video_facade},
    control::{run_screen, run_send, run_status, run_stop, run_wait},
    integrity,
    mcp::run_mcp,
    output::ResponseWriter,
    owned,
    recording::{run_geometry_probe, run_record, run_restart},
    registry,
    scenario::run_test,
    sdk_server::run_sdk_server,
    tmux::dispatch as dispatch_tmux,
    tooling::{run_completions, run_doctor, run_schemas, run_version},
};
use miette::Result;
use std::process::ExitCode;

pub(super) fn dispatch(response: &mut ResponseWriter, command: Command) -> Result<ExitCode> {
    return match command {
        Command::Record(arguments) => run_record(response, arguments),
        Command::Screen(arguments) => run_screen(response, arguments),
        Command::Send(arguments) => run_send(response, arguments),
        Command::Wait(arguments) => run_wait(response, arguments),
        Command::Status(arguments) => run_status(response, arguments),
        Command::Stop(arguments) => run_stop(response, arguments),
        Command::Restart(arguments) => run_restart(response, arguments),
        Command::Analyze(arguments) => dispatch_analyze(response, arguments),
        Command::Screenshot(arguments) => run_screenshot(response, arguments),
        Command::Video(arguments) => run_video_facade(response, arguments),
        Command::Bundle(arguments) => run_bundle(response, arguments),
        Command::Test(arguments) => run_test(response, arguments),
        Command::Tmux(arguments) => dispatch_tmux(response, arguments.command),
        Command::Doctor(arguments) => run_doctor(response, arguments),
        Command::Completion(arguments) => run_completions(response, arguments),
        Command::Mcp => run_mcp(),
        Command::SdkServer => run_sdk_server(),
        Command::Schema(arguments) => run_schemas(response, arguments),
        Command::Version(arguments) => run_version(response, arguments),
        Command::Playback(arguments) => run_playback(arguments),
        Command::PlaybackWorker(arguments) => run_playback_worker(arguments),
        Command::OwnedProcess(arguments) => owned::run(arguments),
        Command::GeometryProbe(arguments) => run_geometry_probe(arguments),
        Command::Artifact(arguments) => artifacts::dispatch(response, arguments.command),
        Command::Session(arguments) => registry::dispatch(response, arguments.command),
        Command::Integrity(arguments) => integrity::dispatch(response, arguments.command),
        Command::LastAction(arguments) => run_last_action_facade(response, arguments),
    };
}
