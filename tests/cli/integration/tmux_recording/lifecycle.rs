use super::*;

#[test]
fn tmux_kill_pane_closes_canonical_recording() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::new(&temporary, "/bin/sh");
    let session = server.pane_session("%0");
    let killed = server
        .command()
        .args(["kill-pane", "-t", "%0"])
        .status()
        .expect("kill pane");
    assert!(killed.success(), "tmux did not kill the pane");
    wait_for_path(&session.join("completion.json"));
    let log = EventLog::read(session.join("events.jsonl")).expect("killed pane log");
    validate_completed_log(&session, &log).expect("killed pane completed evidence");
    assert!(
        matches!(
            log.events.last().map(|record| return &record.event),
            Some(Event::Exit { .. })
        ),
        "killed pane evidence must close with a canonical child exit"
    );
}

#[test]
fn tmux_live_enable_grandfathers_existing_panes_and_records_new_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let original_default_command = "printf 'TMUX_DEFAULT_COMMAND\\n'; exec /bin/sh";
    let configured = server
        .command()
        .args([
            "set-option",
            "-g",
            "default-command",
            original_default_command,
        ])
        .status()
        .expect("configure original tmux default command");
    assert!(configured.success(), "tmux default command was not set");
    let original_pid = trim_newline(
        server
            .command()
            .args(["display-message", "-p", "-t", "%0", "#{pane_pid}"])
            .output()
            .expect("query existing pane")
            .stdout,
    );

    let mut enable = Command::cargo_bin("termtrace").expect("binary should build");
    let enabled = enable
        .arg("tmux")
        .arg("enable")
        .arg("--tmux")
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--output-root"])
        .arg(temporary.path())
        .args(["--ghostty-profile", "off"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let enabled = compact_json_line(&enabled);
    assert_eq!(enabled["existing_panes"], 1);
    assert_eq!(enabled["existing_unrecorded_panes"], 1);
    assert_eq!(enabled["retroactive"], false);
    server.wait_for_process_marker("%0", "@termtrace-grandfathered-pid");

    let recorded_pane = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&recorded_pane);
    let sent = server
        .command()
        .args(["send-keys", "-t", recorded_pane.as_str(), "exit", "Enter"])
        .status()
        .expect("close recorded pane");
    assert!(sent.success(), "recorded pane did not accept input");
    wait_for_path(&recording.join("completion.json"));
    let surviving_pid = trim_newline(
        server
            .command()
            .args(["display-message", "-p", "-t", "%0", "#{pane_pid}"])
            .output()
            .expect("query grandfathered pane after recorded workflow")
            .stdout,
    );
    assert_eq!(surviving_pid, original_pid);
    let manifest = read_manifest(&recording).expect("default-command recording manifest");
    assert_eq!(
        manifest
            .tmux_pane
            .expect("tmux pane provenance")
            .launch_kind,
        TmuxPaneLaunchKind::DefaultCommand
    );
    let log = EventLog::read(recording.join("events.jsonl")).expect("default-command event log");
    let output = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::Output { data } => return Some(data.as_bytes()),
            _ => return None,
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert!(
        output
            .windows(b"TMUX_DEFAULT_COMMAND".len())
            .any(|window| return window == b"TMUX_DEFAULT_COMMAND")
    );

    let status = server.wait_for_status("live-enable coverage to settle", |status| {
        return status["unrecorded_panes"] == 1;
    });
    assert_eq!(status["enabled"], true);

    let mut disable = Command::cargo_bin("termtrace").expect("binary should build");
    disable
        .args(["tmux", "disable", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""default_command_restored":true"#,
        ));
    let restored = server
        .command()
        .args(["show-options", "-gv", "default-command"])
        .output()
        .expect("query restored default command");
    assert!(restored.status.success());
    assert_eq!(
        trim_newline(restored.stdout),
        original_default_command.as_bytes()
    );
    assert!(server.pane_exists("%0"));
}
#[test]
fn tmux_new_recording_allocates_a_socket_instead_of_reusing_default_server() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged_default(&temporary, "/bin/sh");
    let root = temporary.path().to_str().expect("UTF-8 temporary path");
    let tmux = server.binary.to_str().expect("UTF-8 tmux path");
    let tmux_tmpdir = server
        .tmux_tmpdir
        .as_ref()
        .expect("default server socket root");
    let existing_pid = String::from_utf8(trim_newline(
        server
            .command()
            .args(["display-message", "-p", "#{pid}"])
            .output()
            .expect("query existing server PID")
            .stdout,
    ))
    .expect("UTF-8 server PID");
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("TMUX_TMPDIR", tmux_tmpdir)
        .env_remove("TMUX")
        .args([
            "tmux",
            "record",
            "isolated",
            "--new",
            "--tmux",
            tmux,
            "--output-root",
            root,
            "--ghostty-profile",
            "off",
            "--",
            tmux,
            "new-session",
            "-s",
            "aggregate",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let started: serde_json::Value =
        serde_json::from_slice(&output).expect("aggregate start response");
    let manifest_path = started["manifest"]
        .as_str()
        .map(PathBuf::from)
        .expect("aggregate manifest path");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).expect("read aggregate manifest"))
            .expect("aggregate manifest JSON");
    let allocated_socket_bytes: ByteString =
        serde_json::from_value(manifest["socket_name"].clone())
            .expect("allocated socket name bytes");
    let allocated_socket = String::from_utf8(allocated_socket_bytes.into_bytes())
        .expect("UTF-8 allocated socket name");
    assert_ne!(allocated_socket, "default");
    assert!(
        allocated_socket.starts_with("termtrace-recording-"),
        "unexpected allocated socket name: {allocated_socket}"
    );
    assert_ne!(
        manifest["server_pid"]
            .as_u64()
            .map(|pid| return pid.to_string()),
        Some(existing_pid)
    );
    assert!(
        !server
            .command()
            .args(["has-session", "-t", "aggregate"])
            .status()
            .expect("query existing default server")
            .success(),
        "new aggregate session was created on the existing default server"
    );

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("TMUX_TMPDIR", tmux_tmpdir)
        .env_remove("TMUX")
        .args(["tmux", "stop", "isolated", "--output-root", root, "--wait"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no server running").not());
    let _ = StdCommand::new(&server.binary)
        .args(["-L", allocated_socket.as_str(), "kill-server"])
        .env("TMUX_TMPDIR", tmux_tmpdir)
        .env_remove("TMUX")
        .status();
}

#[test]
fn tmux_aggregate_rejects_a_replacement_server_on_the_same_socket() {
    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");
    let binary = tmux_binary();
    let socket = format!("termtrace-identity-test-{}", uuid::Uuid::now_v7());
    let root = temporary.path().to_str().expect("UTF-8 temporary path");
    let tmux = binary.to_str().expect("UTF-8 tmux path");
    let started = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args([
            "tmux",
            "record",
            "identity",
            "--new",
            "-L",
            socket.as_str(),
            "--tmux",
            tmux,
            "--output-root",
            root,
            "--ghostty-profile",
            "off",
            "--",
            tmux,
            "new-session",
            "-s",
            "original",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: serde_json::Value =
        serde_json::from_slice(&started).expect("aggregate start response");
    let manifest_path = response["manifest"]
        .as_str()
        .map(PathBuf::from)
        .expect("aggregate manifest path");
    let deadline = Instant::now() + Duration::from_secs(5);
    let original = loop {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read aggregate manifest"))
                .expect("aggregate manifest JSON");
        if value["server_pid"].is_number() && value["server_socket_inode"].is_number() {
            break value;
        }
        assert!(
            Instant::now() < deadline,
            "aggregate manifest did not bind its original server identity"
        );
        thread::sleep(Duration::from_millis(20));
    };

    let killed = StdCommand::new(&binary)
        .args(["-L", socket.as_str(), "kill-server"])
        .status()
        .expect("stop original tmux server");
    assert!(killed.success());
    wait_for_tmux_server_exit(
        original["server_pid"]
            .as_u64()
            .expect("original tmux server PID"),
    );
    let replacement = StdCommand::new(&binary)
        .args([
            "-L",
            socket.as_str(),
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "impostor",
            "/bin/sleep",
            "30",
        ])
        .status()
        .expect("start replacement tmux server");
    assert!(replacement.success());

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "status", "identity", "--output-root", root])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tmux server identity changed"));
    let preserved: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read preserved manifest"))
            .expect("preserved aggregate manifest JSON");
    assert_eq!(preserved["server_pid"], original["server_pid"]);
    assert_eq!(
        preserved["server_socket_inode"],
        original["server_socket_inode"]
    );
    assert!(preserved["panes"].as_array().is_some_and(|panes| {
        return panes
            .iter()
            .all(|pane| return pane["session_name"] != "impostor");
    }));
    let _ = StdCommand::new(&binary)
        .args(["-L", socket.as_str(), "kill-server"])
        .status();
}
