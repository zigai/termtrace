use crate::support::*;

struct PrivateTmux {
    binary: PathBuf,
    socket: String,
    session: String,
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "tmux test helpers stop immediately when an isolated server violates its contract"
)]
impl PrivateTmux {
    fn new(temporary: &TempDir, shell: &str) -> Option<Self> {
        return Self::new_with_config_args(temporary, shell, &[]);
    }

    fn new_with_config_args(
        temporary: &TempDir,
        shell: &str,
        config_arguments: &[&str],
    ) -> Option<Self> {
        let binary = tmux_binary()?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure tmux test root");
        let socket = format!("termtrace-test-{}", uuid::Uuid::now_v7());
        let config = temporary.path().join("termtrace.tmux.conf");
        let configured_tmux = temporary.path().join("tmux executable alias");
        symlink(&binary, &configured_tmux).expect("create tmux executable alias");
        let mut generate = Command::cargo_bin("termtrace").expect("binary should build");
        generate.args([
            "tmux",
            "config",
            "--output-root",
            temporary.path().to_str().expect("UTF-8 temporary path"),
            "--ghostty-profile",
            "off",
            "--tmux",
            configured_tmux
                .to_str()
                .expect("UTF-8 tmux executable alias"),
        ]);
        let generated = generate
            .args(config_arguments)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let mut configuration = b"set-option -g default-terminal tmux-256color\n".to_vec();
        configuration.extend_from_slice(&generated);
        fs::write(&config, configuration).expect("write generated tmux config");
        let status = StdCommand::new(&binary)
            .args(["-L", socket.as_str(), "-f"])
            .arg(&config)
            .args([
                "new-session",
                "-d",
                "-x",
                "80",
                "-y",
                "24",
                "-s",
                "recorded",
            ])
            .env("SHELL", shell)
            .status()
            .expect("start private tmux server");
        assert!(status.success(), "private tmux server did not start");
        return Some(Self {
            binary,
            socket,
            session: "recorded".to_owned(),
        });
    }

    fn unmanaged(temporary: &TempDir, shell: &str) -> Option<Self> {
        let binary = tmux_binary()?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure tmux test root");
        let socket = format!("termtrace-test-{}", uuid::Uuid::now_v7());
        let status = StdCommand::new(&binary)
            .args([
                "-L",
                socket.as_str(),
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-x",
                "80",
                "-y",
                "24",
                "-s",
                "existing",
                "/bin/sleep",
                "30",
            ])
            .env("SHELL", shell)
            .status()
            .expect("start unmanaged private tmux server");
        assert!(status.success(), "private tmux server did not start");
        return Some(Self {
            binary,
            socket,
            session: "existing".to_owned(),
        });
    }

    fn command(&self) -> StdCommand {
        let mut command = StdCommand::new(&self.binary);
        command.args(["-L", self.socket.as_str()]);
        return command;
    }

    fn pane_session(&self, pane: &str) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = self
                .command()
                .args(["show-options", "-pqv", "-t", pane, "@termtrace-session"])
                .output()
                .expect("query pane recording");
            let value = trim_newline(output.stdout);
            if output.status.success() && !value.is_empty() {
                return PathBuf::from(OsString::from_vec(value));
            }
            assert!(
                Instant::now() < deadline,
                "tmux pane did not publish its recording path"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn pane_option(&self, pane: &str, option: &str) -> Vec<u8> {
        let output = self
            .command()
            .args(["show-options", "-pqv", "-t", pane, option])
            .output()
            .expect("query pane option");
        assert!(
            output.status.success(),
            "could not query {option} for {pane}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return trim_newline(output.stdout);
    }

    fn wait_for_process_marker(&self, pane: &str, option: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pid = trim_newline(
                self.command()
                    .args(["display-message", "-p", "-t", pane, "#{pane_pid}"])
                    .output()
                    .expect("query pane PID")
                    .stdout,
            );
            if self.pane_option(pane, option) == pid {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "tmux pane {pane} did not publish {option}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn configured_executable(&self) -> String {
        let output = self
            .command()
            .args(["show-options", "-gqv", "@termtrace-executable"])
            .output()
            .expect("query configured Termtrace executable");
        assert!(output.status.success());
        return String::from_utf8(trim_newline(output.stdout)).expect("UTF-8 executable");
    }

    fn pane_exists(&self, pane: &str) -> bool {
        return self
            .command()
            .args(["display-message", "-p", "-t", pane, "#{pane_id}"])
            .output()
            .is_ok_and(|output| {
                return output.status.success() && trim_newline(output.stdout) == pane.as_bytes();
            });
    }

    fn wait_for_pane_absent(&self, pane: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.pane_exists(pane) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if self.pane_exists(pane) {
            let panes = self
                .command()
                .args([
                    "list-panes",
                    "-a",
                    "-F",
                    "#{pane_id} pid=#{pane_pid} command=#{pane_current_command} start=#{pane_start_command}",
                ])
                .output()
                .expect("diagnose pane list");
            let supervisor = self
                .command()
                .args(["show-options", "-gqv", "@termtrace-supervisor-pid"])
                .output()
                .expect("diagnose supervisor");
            panic!(
                "unrecorded pane {pane} remained alive; panes={}; supervisor={}",
                String::from_utf8_lossy(&panes.stdout),
                String::from_utf8_lossy(&supervisor.stdout),
            );
        }
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = self
            .command()
            .arg("kill-server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
fn tmux_default_pane_is_a_complete_termtrace_recording() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) = PrivateTmux::new(&temporary, "/bin/sh") else {
        return;
    };
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
    thread::sleep(Duration::from_millis(50));

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
            .windows(b"TMUX_RECORDED".len())
            .any(|window| return window == b"TMUX_RECORDED"),
        "recorded output omitted the pane command"
    );
}

#[test]
fn tmux_strict_mode_blocks_creation_and_respawn_bypasses() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) = PrivateTmux::new(&temporary, "/bin/sh") else {
        return;
    };

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
fn tmux_explicit_scope_records_only_forced_or_explicit_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };

    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");
    assert!(server.pane_option("%0", "@termtrace-session").is_empty());

    let ordinary = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&ordinary, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(
        server.pane_exists(&ordinary),
        "strict enforcement closed an intentionally excluded pane"
    );
    assert!(
        server
            .pane_option(&ordinary, "@termtrace-session")
            .is_empty()
    );

    let executable = server.configured_executable();
    let forced = tmux_new_window(&server, &[executable.as_str(), "tmux", "run", "--force"]);
    let recording = server.pane_session(&forced);
    let sent = server
        .command()
        .args(["send-keys", "-t", forced.as_str(), "exit", "Enter"])
        .status()
        .expect("close forced recording");
    assert!(sent.success(), "forced pane did not accept input");
    wait_for_path(&recording.join("completion.json"));
    assert_eq!(
        read_manifest(&recording)
            .expect("forced pane manifest")
            .tmux_pane
            .expect("tmux pane provenance")
            .launch_kind,
        TmuxPaneLaunchKind::DefaultShell
    );
    assert!(server.pane_exists(&ordinary));
}

#[test]
fn tmux_session_scope_records_only_the_selected_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--session", "recorded"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["changed"], true);
    assert_eq!(changed["future_processes_only"], true);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["session_ids"][0], "$0");

    let selected = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&selected);

    let excluded = tmux_new_session(&server, "excluded", &[]);
    server.wait_for_process_marker(&excluded, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(server.pane_exists(&excluded));
    assert!(
        server
            .pane_option(&excluded, "@termtrace-session")
            .is_empty()
    );

    let sent = server
        .command()
        .args(["send-keys", "-t", selected.as_str(), "exit", "Enter"])
        .status()
        .expect("close session-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&excluded));
}

#[test]
fn tmux_pane_scope_records_that_pane_after_its_process_restarts() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");
    let other = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&other, "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--pane", "%0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["pane_ids"][0], "%0");
    assert!(
        server.pane_option("%0", "@termtrace-session").is_empty(),
        "scope changes must not attach to a running pane process"
    );

    let wrapper = server
        .command()
        .args(["show-options", "-gv", "default-command"])
        .output()
        .expect("query managed default command");
    assert!(wrapper.status.success());
    let restarted = server
        .command()
        .args(["respawn-pane", "-k", "-t", "%0"])
        .arg(OsString::from_vec(trim_newline(wrapper.stdout)))
        .status()
        .expect("restart selected pane through managed wrapper");
    assert!(restarted.success());
    let recording = server.pane_session("%0");
    let sent = server
        .command()
        .args(["send-keys", "-t", "%0", "exit", "Enter"])
        .status()
        .expect("close pane-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&other));
    assert!(server.pane_option(&other, "@termtrace-session").is_empty());
}

#[test]
fn tmux_window_scope_records_new_panes_only_in_that_window() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--window", "recorded:0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["window_ids"][0], "@0");

    let selected = tmux_split_window(&server, "%0");
    let recording = server.pane_session(&selected);
    let excluded = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&excluded, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(server.pane_exists(&excluded));

    let sent = server
        .command()
        .args(["send-keys", "-t", selected.as_str(), "exit", "Enter"])
        .status()
        .expect("close window-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&excluded));
}

#[test]
fn tmux_kill_pane_closes_canonical_recording() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) = PrivateTmux::new(&temporary, "/bin/sh") else {
        return;
    };
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
    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::HostError { code, .. } if code == "recorder_signal_received"
        );
    }));
}

#[test]
fn tmux_live_enable_grandfathers_existing_panes_and_records_new_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) = PrivateTmux::unmanaged(&temporary, "/bin/sh") else {
        return;
    };
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

    thread::sleep(Duration::from_millis(900));
    assert!(
        server.pane_exists("%0"),
        "live enable closed the existing pane"
    );
    let surviving_pid = trim_newline(
        server
            .command()
            .args(["display-message", "-p", "-t", "%0", "#{pane_pid}"])
            .output()
            .expect("query grandfathered pane")
            .stdout,
    );
    assert_eq!(surviving_pid, original_pid);

    let recorded_pane = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&recorded_pane);
    let sent = server
        .command()
        .args(["send-keys", "-t", recorded_pane.as_str(), "exit", "Enter"])
        .status()
        .expect("close recorded pane");
    assert!(sent.success(), "recorded pane did not accept input");
    wait_for_path(&recording.join("completion.json"));
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

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    status
        .args(["tmux", "status", "--json", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""enabled":true"#))
        .stdout(predicate::str::contains(r#""unrecorded_panes":1"#));

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

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when its isolated command cannot be inspected"
)]
fn tmux_new_window(server: &PrivateTmux, command: &[&str]) -> String {
    let mut create = server.command();
    create
        .args(["new-window", "-d", "-P", "-F", "#{pane_id}", "-t"])
        .arg(format!("{}:", server.session));
    create.args(command);
    let output = create.output().expect("create tmux window");
    assert!(
        output.status.success(),
        "tmux window creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    return String::from_utf8(trim_newline(output.stdout)).expect("UTF-8 pane ID");
}

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when its isolated command cannot be inspected"
)]
fn tmux_split_window(server: &PrivateTmux, target: &str) -> String {
    let output = server
        .command()
        .args(["split-window", "-d", "-P", "-F", "#{pane_id}", "-t", target])
        .output()
        .expect("split tmux window");
    assert!(
        output.status.success(),
        "tmux window split failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    return String::from_utf8(trim_newline(output.stdout)).expect("UTF-8 pane ID");
}

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when its isolated command cannot be inspected"
)]
fn tmux_new_session(server: &PrivateTmux, name: &str, command: &[&str]) -> String {
    let mut create = server.command();
    create.args(["new-session", "-d", "-P", "-F", "#{pane_id}", "-s", name]);
    create.args(command);
    let output = create.output().expect("create tmux session");
    assert!(
        output.status.success(),
        "tmux session creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    return String::from_utf8(trim_newline(output.stdout)).expect("UTF-8 pane ID");
}

fn trim_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    return bytes;
}

fn tmux_binary() -> Option<PathBuf> {
    let binary = if Path::new("/usr/bin/tmux").is_file() {
        PathBuf::from("/usr/bin/tmux")
    } else {
        PathBuf::from("tmux")
    };
    return StdCommand::new(&binary)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| return status.success())
        .then_some(binary);
}
