use crate::support::*;
use serde::Deserialize;

use termtrace_evidence::artifact::verify_artifact_members;
use termtrace_model::artifact::tmux::TmuxBundleManifest;
mod bundle;
mod coverage;
mod lifecycle;
mod media;
mod scope;

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
        "../../../fixtures/regression-workflows/omp-extensions-tmux.json"
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
