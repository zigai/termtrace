use super::*;

pub(super) fn dispatch(command: Command) -> Result<ExitCode> {
    match command {
        Command::Record(arguments) => return run_record(arguments),
        Command::Test(arguments) => return run_test(arguments),
        Command::Start(arguments) => return run_start(arguments),
        Command::Restart(arguments) => return run_restart(arguments),
        Command::Status(arguments) => return run_status(arguments),
        Command::Send(arguments) => return run_send(arguments),
        Command::Stop(arguments) => return run_stop(arguments),
        Command::Tmux(arguments) => return dispatch_tmux(arguments.command),
        Command::Control(arguments) => return dispatch_control(arguments.command),
        Command::Analyze(arguments) => return dispatch_analyze(arguments.command),
        Command::Artifact(arguments) => return dispatch_artifact(arguments.command),
        Command::Registry(arguments) => return dispatch_registry(arguments.command),
        Command::Integrity(arguments) => return dispatch_integrity(arguments.command),
        Command::Tooling(arguments) => return dispatch_tooling(arguments.command),
        Command::Playback(arguments) => return run_playback(arguments),
        Command::GeometryProbe(arguments) => {
            run_probe(&arguments.out)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Show(arguments) => return run_show(arguments),
        Command::Wait(arguments) => return run_wait(arguments),
        Command::WaitIdle(arguments) => return run_wait_idle(arguments),
    }
}

pub(super) fn dispatch_tmux(command: TmuxSubcommand) -> Result<ExitCode> {
    return match command {
        TmuxSubcommand::Config(arguments) => run_tmux_config(arguments),
        TmuxSubcommand::Enable(arguments) => run_tmux_enable(arguments),
        TmuxSubcommand::Disable(arguments) => run_tmux_disable(arguments),
        TmuxSubcommand::Status(arguments) => run_tmux_status(arguments),
        TmuxSubcommand::Scope(arguments) => run_tmux_scope(arguments),
        TmuxSubcommand::Run(arguments) => run_tmux_pane(arguments),
        TmuxSubcommand::Guard(arguments) => run_tmux_guard(arguments),
        TmuxSubcommand::Supervise => run_tmux_supervisor(),
    };
}

pub(super) fn dispatch_control(command: ControlSubcommand) -> Result<ExitCode> {
    match command {
        ControlSubcommand::Ping(arguments) => {
            return run_control(arguments.session, ControlCommand::Ping);
        }
        ControlSubcommand::Keys => return run_keys(),
        ControlSubcommand::Focus(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Focus {
                    focused: matches!(arguments.direction, FocusDirection::In),
                    strict: arguments.strict,
                },
                arguments.expected_presented_state_hash,
            );
        }
        ControlSubcommand::Pointer(arguments) => return run_pointer(arguments),
        ControlSubcommand::Activate(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Activate {
                    text: arguments.text,
                    unique: arguments.unique,
                    strict: arguments.strict,
                },
                arguments.expected_presented_state_hash,
            );
        }
        ControlSubcommand::Resize(arguments) => return run_resize(arguments),
        ControlSubcommand::Signal(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Signal {
                    signal: arguments.signal.into(),
                },
                arguments.expected_presented_state_hash,
            );
        }
        ControlSubcommand::Mark(arguments) => {
            return run_control_guarded(
                arguments.session,
                ControlCommand::Mark {
                    label: arguments.label,
                },
                arguments.expected_presented_state_hash,
            );
        }
    }
}

pub(super) fn dispatch_analyze(command: AnalyzeSubcommand) -> Result<ExitCode> {
    match command {
        AnalyzeSubcommand::Index(arguments) => return run_index(arguments),
        AnalyzeSubcommand::Inspect(arguments) => return run_inspect(arguments),
        AnalyzeSubcommand::Timeline(arguments) => return run_timeline(arguments),
        AnalyzeSubcommand::Diff(arguments) => return run_diff(arguments),
        AnalyzeSubcommand::Explain(arguments) => return run_explain(arguments),
        AnalyzeSubcommand::Diagnose(arguments) => return run_diagnose(arguments),
        AnalyzeSubcommand::Review(arguments) => return run_review(arguments),
        AnalyzeSubcommand::Output(arguments) => return run_output(arguments),
        AnalyzeSubcommand::Events(arguments) => return run_events(arguments),
        AnalyzeSubcommand::Markers(arguments) => return run_markers(arguments),
        AnalyzeSubcommand::Cache(arguments) => return run_cache(arguments),
    }
}

pub(super) fn dispatch_artifact(command: ArtifactSubcommand) -> Result<ExitCode> {
    match command {
        ArtifactSubcommand::Capture(arguments) => return run_capture(arguments),
        ArtifactSubcommand::Save(arguments) => return run_save(arguments),
        ArtifactSubcommand::Screenshot(arguments) => return run_screenshot(arguments),
        ArtifactSubcommand::Video(arguments) => return run_video(arguments),
        ArtifactSubcommand::Bundle(arguments) => {
            return match arguments.command {
                ArtifactBundleSubcommand::Create(arguments) => run_bundle(arguments),
                ArtifactBundleSubcommand::Verify(arguments) => run_bundle_verify(arguments),
            };
        }
        ArtifactSubcommand::Asciicast(arguments) => return run_export_asciicast(arguments),
    }
}

pub(super) fn dispatch_registry(command: RegistrySubcommand) -> Result<ExitCode> {
    match command {
        RegistrySubcommand::Register(arguments) => return run_register(arguments),
        RegistrySubcommand::Resolve(arguments) => return run_resolve(arguments),
        RegistrySubcommand::List(arguments) => return run_list(arguments),
        RegistrySubcommand::Remove(arguments) => return run_remove(arguments),
        RegistrySubcommand::Gc(arguments) => return run_gc(arguments),
    }
}

pub(super) fn dispatch_integrity(command: IntegritySubcommand) -> Result<ExitCode> {
    match command {
        IntegritySubcommand::Verify(arguments) => return run_verify(arguments),
        IntegritySubcommand::Sign(arguments) => return run_sign(arguments),
        IntegritySubcommand::Recover(arguments) => return run_recover(arguments),
    }
}

pub(super) fn dispatch_tooling(command: ToolingSubcommand) -> Result<ExitCode> {
    match command {
        ToolingSubcommand::Doctor(arguments) => return run_doctor(arguments),
        ToolingSubcommand::Client(arguments) => return run_client(arguments),
        ToolingSubcommand::Driver => return run_driver(),
        ToolingSubcommand::Schemas(arguments) => return run_schemas(arguments),
        ToolingSubcommand::Completions(arguments) => return run_completions(arguments),
        ToolingSubcommand::Version(arguments) => return run_version(arguments),
    }
}
