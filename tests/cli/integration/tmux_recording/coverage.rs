use super::*;

#[test]
fn tmux_default_pane_is_a_complete_termtrace_recording() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::new(&temporary, "/bin/sh");
    let pane = "%0";
    let session = server.pane_session(pane);
    let manual_size = server
        .command()
        .args([
            "set-option",
            "-w",
            "-t",
            "recorded:0",
            "window-size",
            "manual",
        ])
        .status()
        .expect("set manual tmux window size");

    assert!(manual_size.success(), "tmux window size mode was not set");

    let resized = server
        .command()
        .args(["resize-window", "-t", "recorded:0", "-x", "100", "-y", "30"])
        .status()
        .expect("resize tmux window");

    assert!(resized.success(), "tmux window was not resized");

    wait_for_recorded_resize(&session, 100, 30);

    let sent = server
        .command()
        .args([
            "send-keys",
            "-t",
            pane,
            "printf 'TMUX_RECORDED\\n'; exit",
            "Enter",
        ])
        .output()
        .expect("send pane input");

    assert!(
        sent.status.success(),
        "tmux input was not accepted: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    wait_for_path(&session.join("completion.json"));

    let manifest = read_manifest(&session).expect("tmux session manifest");
    let tmux = manifest.tmux_pane.expect("tmux pane provenance");

    assert_eq!(tmux.pane_id, pane);
    assert_eq!(tmux.session_id, "$0");
    assert_eq!(tmux.window_id, "@0");
    assert_eq!(tmux.launch_kind, TmuxPaneLaunchKind::DefaultShell);
    assert_eq!(manifest.geometry.cols, 80);
    assert_eq!(manifest.geometry.rows, 24);
    assert_eq!(
        manifest.geometry_provenance.source,
        GeometrySource::Inherited
    );

    let log = EventLog::read(session.join("events.jsonl")).expect("tmux event log");
    assert_eq!(log.header.term, "tmux-256color");
    validate_completed_log(&session, &log).expect("valid completed tmux evidence");

    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::InputIntent {
                source: InputSource::Human,
                ..
            }
        );
    }));
    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::ResizeIntent {
                cols: 100,
                rows: 30,
                ..
            }
        );
    }));

    let output = canonical_output_bytes(&log);
    assert!(
        output
            .windows(b"TMUX_RECORDED".len())
            .any(|window| return window == b"TMUX_RECORDED"),
        "recorded output omitted the pane command"
    );
}

#[test]
fn tmux_strict_mode_blocks_creation_and_respawn_bypasses() {
    let temporary = TempDir::new().expect("temporary directory");
    let coverage = tmux_workflow_fixture().tmux_coverage;
    let setup_args = coverage
        .strict_setup_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    let server = PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &setup_args);
    let bypass = tmux_new_window(&server, &["/bin/sleep", "30"]);
    server.wait_for_pane_absent(&bypass);

    let configured_executable = server
        .command()
        .args(["show-options", "-gqv", "@termtrace-executable"])
        .output()
        .expect("query configured Termtrace executable");

    assert!(configured_executable.status.success());

    let configured_executable =
        String::from_utf8(trim_newline(configured_executable.stdout)).expect("UTF-8 executable");
    let explicit = tmux_new_window(
        &server,
        &[
            configured_executable.as_str(),
            "tmux",
            "run",
            "--",
            "/bin/sh",
        ],
    );

    let explicit_recording = server.pane_session(&explicit);
    let sent = server
        .command()
        .args([
            "send-keys",
            "-t",
            explicit.as_str(),
            "printf 'EXPLICIT_WRAPPER\\n'; exit",
            "Enter",
        ])
        .status()
        .expect("drive explicit wrapped pane");

    assert!(sent.success(), "explicit wrapped pane did not accept input");

    wait_for_path(&explicit_recording.join("completion.json"));
    assert_eq!(
        read_manifest(&explicit_recording)
            .expect("explicit wrapper manifest")
            .tmux_pane
            .expect("tmux pane provenance")
            .launch_kind,
        TmuxPaneLaunchKind::ExplicitArgv
    );

    let wrapped = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&wrapped);
    let respawned = server
        .command()
        .args([
            "respawn-pane",
            "-k",
            "-t",
            wrapped.as_str(),
            "/bin/sleep",
            "30",
        ])
        .status()
        .expect("respawn pane");

    assert!(respawned.success(), "tmux did not respawn the pane");

    server.wait_for_pane_absent(&wrapped);

    wait_for_path(&recording.join("completion.json"));
    let log = EventLog::read(recording.join("events.jsonl")).expect("interrupted pane log");
    validate_completed_log(&recording, &log).expect("respawn closed canonical evidence");
    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::HostError { code, .. } if code == "recorder_signal_received"
        );
    }));
}

#[test]
fn tmux_default_warn_coverage_never_terminates_uncovered_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let workflow = tmux_workflow_fixture();
    let setup_args = workflow
        .tmux_coverage
        .warn_setup_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    let server = PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &setup_args);
    let bypass = tmux_new_window(&server, &["/bin/sleep", "30"]);
    let status = server.wait_for_status("warn coverage to report the uncovered pane", |status| {
        return status["strict"] == false
            && status["unrecorded_panes"].as_u64().unwrap_or_default() >= 1;
    });

    assert!(
        server.pane_exists(&bypass),
        "default warn coverage terminated an uncovered pane"
    );
    assert_eq!(status["strict"], false);
    assert!(
        status["unrecorded_panes"].as_u64().unwrap_or_default() >= 1,
        "warn coverage did not report its uncovered pane: {status}"
    );

    let recorded = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&recorded);
    let launch = format!("exec /usr/bin/python3 -c \"{}\"", workflow.program);

    let typed = server
        .command()
        .args(["send-keys", "-t", recorded.as_str(), "-l", launch.as_str()])
        .status()
        .expect("type long tmux pane command");

    assert!(typed.success());

    let entered = server
        .command()
        .args(["send-keys", "-t", recorded.as_str(), "Enter"])
        .status()
        .expect("launch long tmux pane command");

    assert!(entered.success());

    wait_for_output(
        &recording,
        workflow.ready.as_bytes(),
        Duration::from_secs(15),
    )
    .expect("long tmux pane ready output");

    let killed = server
        .command()
        .args(["kill-pane", "-t", recorded.as_str()])
        .status()
        .expect("close long tmux pane");

    assert!(killed.success());

    wait_for_completion(&recording, Duration::from_secs(30))
        .expect("long tmux pane completion evidence");

    assert!(
        server.pane_exists(&bypass),
        "warn coverage terminated its uncovered pane during a recorded workflow"
    );

    assert_late_checkpoint_inspection(&recording);
}
