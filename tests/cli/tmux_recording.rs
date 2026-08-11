use crate::support::*;
use serde::Deserialize;

use termtrace::{artifact::verify_artifact_members, tmux_bundle::TmuxBundleManifest};

#[derive(Deserialize)]
struct TmuxWorkflowFixture {
    program: String,
    ready: String,
    tmux_coverage: TmuxCoverageFixture,
}

#[derive(Deserialize)]
struct TmuxCoverageFixture {
    warn_setup_args: Vec<String>,
    strict_setup_args: Vec<String>,
}

#[allow(
    clippy::expect_used,
    reason = "integration fixture helper must stop when its embedded JSON is invalid"
)]
fn tmux_workflow_fixture() -> TmuxWorkflowFixture {
    return serde_json::from_str(include_str!(
        "../fixtures/regression-workflows/omp-extensions-tmux.json"
    ))
    .expect("executable tmux workflow fixture");
}

struct PrivateTmux {
    binary: PathBuf,
    socket: String,
    session: String,
    tmux_tmpdir: Option<PathBuf>,
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
            "--response",
            "full",
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
            tmux_tmpdir: None,
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
            tmux_tmpdir: None,
        });
    }

    fn unmanaged_default(temporary: &TempDir, shell: &str) -> Option<Self> {
        let binary = tmux_binary()?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure isolated tmux test root");
        let tmux_tmpdir = temporary.path().join("tmux-sockets");
        fs::create_dir_all(&tmux_tmpdir).expect("create isolated tmux socket root");
        fs::set_permissions(&tmux_tmpdir, Permissions::from_mode(0o700))
            .expect("secure isolated tmux socket root");
        let status = StdCommand::new(&binary)
            .args([
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
            .env("TMUX_TMPDIR", &tmux_tmpdir)
            .env("SHELL", shell)
            .status()
            .expect("start isolated default tmux server");
        assert!(
            status.success(),
            "isolated default tmux server did not start"
        );
        return Some(Self {
            binary,
            socket: "default".to_owned(),
            session: "existing".to_owned(),
            tmux_tmpdir: Some(tmux_tmpdir),
        });
    }

    fn command(&self) -> StdCommand {
        let mut command = StdCommand::new(&self.binary);
        command.args(["-L", self.socket.as_str()]);
        if let Some(tmux_tmpdir) = &self.tmux_tmpdir {
            command.env("TMUX_TMPDIR", tmux_tmpdir);
        }
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
    let coverage = tmux_workflow_fixture().tmux_coverage;
    let setup_args = coverage
        .strict_setup_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let Some(server) = PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &setup_args) else {
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
fn tmux_default_warn_coverage_never_terminates_uncovered_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let workflow = tmux_workflow_fixture();
    let setup_args = workflow
        .tmux_coverage
        .warn_setup_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let Some(server) = PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &setup_args) else {
        return;
    };

    let bypass = tmux_new_window(&server, &["/bin/sleep", "30"]);
    thread::sleep(Duration::from_millis(1_000));
    assert!(
        server.pane_exists(&bypass),
        "default warn coverage terminated an uncovered pane"
    );

    let status: serde_json::Value = serde_json::from_slice(
        &Command::cargo_bin("termtrace")
            .expect("binary should build")
            .args([
                "tmux",
                "status",
                "-L",
                server.socket.as_str(),
                "--tmux",
                server.binary.to_str().expect("UTF-8 tmux path"),
                "--json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("structured tmux status");
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
    assert_late_checkpoint_inspection(&recording);
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

#[test]
fn tmux_video_uses_shared_optional_presentation_frame_rates() {
    let temporary = TempDir::new().expect("temporary directory");
    let output = temporary.path().join("video.mp4");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "video", "missing", "--fps", "25", "-o"])
        .arg(&output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("video FPS must be 24, 30, or 60"));
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "video", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[default: 30]").not());
}

#[test]
fn tmux_new_recording_allocates_a_socket_instead_of_reusing_default_server() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) = PrivateTmux::unmanaged_default(&temporary, "/bin/sh") else {
        return;
    };
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
        .args(["tmux", "stop", "isolated", "--output-root", root, "--wait"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no server running").not());
    let _ = StdCommand::new(&server.binary)
        .args(["-L", allocated_socket.as_str(), "kill-server"])
        .env("TMUX_TMPDIR", tmux_tmpdir)
        .status();
}

#[test]
fn tmux_aggregate_rejects_a_replacement_server_on_the_same_socket() {
    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");
    let Some(binary) = tmux_binary() else {
        return;
    };
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
    thread::sleep(Duration::from_millis(200));
    assert!(killed.success());
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

#[test]
fn tmux_bundle_contains_the_aggregate_manifest_and_every_canonical_track() {
    use termtrace::tmux_recording::{
        TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION, TmuxCompositeTrack, TmuxRecordingCoverage,
        TmuxRecordingLifecycle, TmuxRecordingManifest,
    };

    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");
    let composite = temporary.path().join("composite");
    let pane = temporary.path().join("pane");
    for (session, output) in [(&composite, "COMPOSITE"), (&pane, "PANE")] {
        let mut record = Command::cargo_bin("termtrace").expect("binary should build");
        record
            .args(["record", "--no-input", "--out"])
            .arg(session)
            .args(["--", "/bin/sh", "-c", &format!("printf {output}")])
            .assert()
            .success();
    }
    let state = temporary.path().join("state");
    let recordings = state.join("tmux-recordings");
    let aggregate = recordings.join("work");
    fs::create_dir_all(&aggregate).expect("aggregate directory");
    for directory in [state.as_path(), recordings.as_path(), aggregate.as_path()] {
        fs::set_permissions(directory, Permissions::from_mode(0o700))
            .expect("private aggregate directory");
    }
    let composite_manifest = read_manifest(&composite).expect("composite manifest");
    let mut manifest = TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: uuid::Uuid::now_v7(),
        name: "work".to_owned(),
        lifecycle: TmuxRecordingLifecycle::Live,
        created_unix_ns: 1,
        updated_unix_ns: 2,
        coverage: TmuxRecordingCoverage::Warn,
        tmux_executable: ByteString::from(b"/usr/bin/tmux".as_slice()),
        socket_name: None,
        socket_path: None,
        server_pid: None,
        server_socket_path: None,
        server_socket_device: None,
        server_socket_inode: None,
        composite_track: TmuxCompositeTrack {
            alias: "tmux-work-composite".to_owned(),
            session_id: Some(composite_manifest.session_id),
            session: Some(ByteString::from(composite.as_os_str().as_bytes())),
            lifecycle: TmuxRecordingLifecycle::Completed,
        },
        panes: Vec::new(),
        pane_tracks: BTreeMap::from([(
            "%1".to_owned(),
            ByteString::from(pane.as_os_str().as_bytes()),
        )]),
        events: Vec::new(),
        coverage_gaps: Vec::new(),
    };
    fs::write(
        aggregate.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("aggregate manifest JSON"),
    )
    .expect("aggregate manifest");

    let rejected = temporary.path().join("live-aggregate-bundle");
    let mut live = Command::cargo_bin("termtrace").expect("binary should build");
    live.args(["tmux", "bundle", "work", "--output-root"])
        .arg(&state)
        .arg("--out")
        .arg(&rejected)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "must be completed before bundling",
        ));
    assert!(!rejected.exists());
    manifest.lifecycle = TmuxRecordingLifecycle::Completed;
    fs::write(
        aggregate.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("completed aggregate manifest JSON"),
    )
    .expect("completed aggregate manifest");

    let bundle = temporary.path().join("aggregate-bundle");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let output = command
        .args(["tmux", "bundle", "work", "--output-root"])
        .arg(&state)
        .arg("--out")
        .arg(&bundle)
        .assert()
        .success();
    let response = compact_json_line(&output.get_output().stdout);
    assert_eq!(response["recording_name"], "work");
    assert_eq!(response["track_count"], 2);
    assert!(bundle.join("aggregate-recording-manifest.json").is_file());
    let bundled: TmuxBundleManifest = serde_json::from_slice(
        &fs::read(bundle.join("manifest.json")).expect("tmux bundle manifest"),
    )
    .expect("tmux bundle manifest JSON");
    assert_eq!(bundled.tracks.len(), 2);
    assert_eq!(bundled.tracks[0].role, "composite");
    assert_eq!(bundled.tracks[1].pane_id.as_deref(), Some("%1"));
    for track in &bundled.tracks {
        assert!(
            bundle
                .join(&track.directory)
                .join("canonical/completion.json")
                .is_file(),
            "missing canonical evidence for {}",
            track.directory
        );
    }
    verify_artifact_members(&bundle, &bundled.members).expect("aggregate member inventory");
}

#[test]
fn tmux_aggregate_manifest_links_composite_and_pane_tracks() {
    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");
    let Some(binary) = tmux_binary() else {
        return;
    };
    let socket = format!("termtrace-aggregate-test-{}", uuid::Uuid::now_v7());
    let root = temporary.path().to_str().expect("UTF-8 temporary path");
    let tmux = binary.to_str().expect("UTF-8 tmux path");
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args([
            "tmux",
            "record",
            "work",
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
            "aggregate",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let started: serde_json::Value =
        serde_json::from_slice(&output).expect("aggregate start response");
    assert_eq!(started["name"], "work");
    assert!(started["composite_track"]["session"].is_string());

    let manifest = temporary.path().join("tmux-recordings/work/manifest.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    let linked = loop {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).expect("read aggregate manifest"))
                .expect("aggregate manifest JSON");
        if value["composite_track"]["session_id"].is_string()
            && value["pane_tracks"]
                .as_object()
                .is_some_and(|tracks| return !tracks.is_empty())
        {
            break value;
        }
        assert!(
            Instant::now() < deadline,
            "aggregate manifest did not link both track layers"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(linked["format"], "termtrace-tmux-recording");
    assert_eq!(linked["version"], 2);
    assert_eq!(linked["coverage"], "warn");
    assert!(linked["events"].as_array().is_some_and(|events| {
        return events
            .iter()
            .any(|event| return event["kind"] == "pane_track_linked");
    }));

    let status: serde_json::Value = serde_json::from_slice(
        &Command::cargo_bin("termtrace")
            .expect("binary should build")
            .args([
                "--response",
                "full",
                "tmux",
                "status",
                "work",
                "--output-root",
                root,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("aggregate status response");
    assert_eq!(status["recording_id"], linked["recording_id"]);
    assert_eq!(
        status["composite_track"]["session_id"],
        linked["composite_track"]["session_id"]
    );
    assert_eq!(status["pane_tracks"], linked["pane_tracks"]);

    let bypass = String::from_utf8(trim_newline(
        StdCommand::new(&binary)
            .args([
                "-L",
                socket.as_str(),
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "/bin/sleep",
                "30",
            ])
            .output()
            .expect("create uncovered aggregate pane")
            .stdout,
    ))
    .expect("UTF-8 pane ID");
    let gap_deadline = Instant::now() + Duration::from_secs(5);
    let uncovered = loop {
        let refreshed: serde_json::Value = serde_json::from_slice(
            &Command::cargo_bin("termtrace")
                .expect("binary should build")
                .args([
                    "--response",
                    "full",
                    "tmux",
                    "status",
                    "work",
                    "--output-root",
                    root,
                ])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("aggregate status with uncovered pane");
        if let Some(gap) = refreshed["coverage_gaps"]
            .as_array()
            .and_then(|gaps| {
                return gaps.iter().find(|gap| {
                    return gap["kind"] == "uncovered_pane"
                        && gap["pane_id"] == bypass
                        && gap["resolved_unix_ns"].is_null();
                });
            })
            .cloned()
        {
            break gap;
        }
        assert!(
            Instant::now() < gap_deadline,
            "aggregate manifest did not retain the uncovered pane"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(uncovered["first_observed_unix_ns"].is_u64());
    assert!(uncovered["last_observed_unix_ns"].is_u64());
    let killed = StdCommand::new(&binary)
        .args(["-L", socket.as_str(), "kill-pane", "-t", bypass.as_str()])
        .status()
        .expect("kill uncovered aggregate pane");
    assert!(killed.success());
    let resolution_deadline = Instant::now() + Duration::from_secs(5);
    let resolved = loop {
        let refreshed: serde_json::Value = serde_json::from_slice(
            &Command::cargo_bin("termtrace")
                .expect("binary should build")
                .args([
                    "--response",
                    "full",
                    "tmux",
                    "status",
                    "work",
                    "--output-root",
                    root,
                ])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("aggregate status after gap resolution");
        let historical = refreshed["coverage_gaps"]
            .as_array()
            .and_then(|gaps| {
                return gaps.iter().find(|gap| {
                    return gap["kind"] == "uncovered_pane" && gap["pane_id"] == bypass;
                });
            })
            .cloned()
            .expect("historical uncovered pane gap");
        if historical["resolved_unix_ns"].is_u64() {
            break (historical, refreshed);
        }
        assert!(
            Instant::now() < resolution_deadline,
            "aggregate gap did not acquire resolution evidence"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        resolved.0["first_observed_unix_ns"],
        uncovered["first_observed_unix_ns"]
    );
    let initial_last_observed = uncovered["last_observed_unix_ns"]
        .as_u64()
        .expect("initial gap last observation");
    let final_last_observed = resolved.0["last_observed_unix_ns"]
        .as_u64()
        .expect("resolved gap last observation");
    let resolved_unix_ns = resolved.0["resolved_unix_ns"]
        .as_u64()
        .expect("gap resolution timestamp");
    assert!(final_last_observed >= initial_last_observed);
    assert!(resolved_unix_ns >= final_last_observed);
    assert!(resolved.1["events"].as_array().is_some_and(|events| {
        return ["coverage_gap_opened", "coverage_gap_resolved"]
            .iter()
            .all(|kind| {
                return events.iter().any(|event| {
                    return event["kind"] == *kind && event["pane_id"] == bypass;
                });
            });
    }));

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "stop", "work", "--output-root", root, "--wait"])
        .assert()
        .success();
    let video = temporary.path().join("aggregate.mp4");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("PATH", "")
        .args([
            "tmux",
            "video",
            "work",
            "--output-root",
            root,
            "--renderer",
            "semantic",
            "--preset",
            "social",
            "-o",
        ])
        .arg(&video)
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""type":"video""#))
        .stdout(predicate::str::contains("Planning video from").not())
        .stderr(predicate::str::contains("Planning video from"))
        .stderr(predicate::str::contains("semantic-video"));
    let stopped: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read stopped aggregate manifest"))
            .expect("stopped aggregate manifest JSON");
    assert_eq!(stopped["lifecycle"], "completed");
    assert_eq!(stopped["composite_track"]["lifecycle"], "completed");
    let _ = StdCommand::new(binary)
        .args(["-L", socket.as_str(), "kill-server"])
        .status();
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
