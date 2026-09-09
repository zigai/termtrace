use crate::cli::command::Command;
use crate::cli::{
    analysis::{
        dispatch as dispatch_analyze, run_diagnose_facade, run_inspect_facade, run_playback,
        run_playback_worker, run_show,
    },
    artifacts::{
        dispatch as dispatch_artifact, run_bug_report_facade, run_bundle, run_screenshot,
        run_video_facade,
    },
    control::{
        dispatch as dispatch_control, run_act, run_screen, run_send, run_status, run_stop, run_wait,
    },
    integrity::dispatch as dispatch_integrity,
    output::ResponseWriter,
    owned,
    recording::{run_geometry_probe, run_record, run_restart, run_start},
    registry::dispatch as dispatch_registry,
    scenario::run_test,
    tmux::dispatch as dispatch_tmux,
    tooling::{dispatch as dispatch_tooling, run_doctor, run_driver, run_version},
};
use miette::Result;
use std::process::ExitCode;

pub(super) fn dispatch(response: &mut ResponseWriter, command: Command) -> Result<ExitCode> {
    return match command {
        Command::Record(arguments) => run_record(response, arguments),
        Command::Test(arguments) => run_test(response, arguments),
        Command::Start(arguments) => run_start(response, arguments),
        Command::Restart(arguments) => run_restart(response, arguments),
        Command::Status(arguments) => run_status(response, arguments),
        Command::Act(arguments) => run_act(response, arguments),
        Command::Send(arguments) => run_send(response, arguments),
        Command::Stop(arguments) => run_stop(response, arguments),
        Command::Tmux(arguments) => dispatch_tmux(response, arguments.command),
        Command::Control(arguments) => dispatch_control(response, arguments.command),
        Command::Analyze(arguments) => dispatch_analyze(response, arguments.command),
        Command::Artifact(arguments) => dispatch_artifact(response, arguments.command),
        Command::Inspect(arguments) => run_inspect_facade(response, arguments),
        Command::Diagnose(arguments) => run_diagnose_facade(response, arguments),
        Command::Screenshot(arguments) => run_screenshot(response, arguments),
        Command::Bundle(arguments) => run_bundle(response, arguments),
        Command::BugReport(arguments) => run_bug_report_facade(response, arguments),
        Command::Video(arguments) => run_video_facade(response, arguments),
        Command::Session(arguments) => dispatch_registry(response, arguments.command),
        Command::Integrity(arguments) => dispatch_integrity(response, arguments.command),
        Command::Tooling(arguments) => dispatch_tooling(response, arguments.command),
        Command::Doctor(arguments) => run_doctor(response, arguments),
        Command::Version(arguments) => run_version(response, arguments),
        Command::Agent => run_driver(),
        Command::Playback(arguments) => run_playback(arguments),
        Command::PlaybackWorker(arguments) => run_playback_worker(arguments),
        Command::OwnedProcess(arguments) => owned::run(arguments),
        Command::GeometryProbe(arguments) => run_geometry_probe(arguments),
        Command::Show(arguments) => run_show(response, arguments),
        Command::Screen(arguments) => run_screen(response, arguments),
        Command::Wait(arguments) => run_wait(response, arguments),
    };
}
