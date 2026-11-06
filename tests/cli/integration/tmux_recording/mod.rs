use crate::support::*;
use serde::Deserialize;
use termtrace_tmux::artifact::{TmuxBundleTrack, TmuxStreamingBundleManifest};

mod bundle;
mod client;
mod coverage;
mod lifecycle;
mod media;
mod scope;
mod setup;

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
    tmux_tmpdir: PathBuf,
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "tmux test helpers stop immediately when an isolated server violates its contract"
)]
impl PrivateTmux {
    fn new(temporary: &TempDir, shell: &str) -> Self {
        return Self::new_with_config_args(temporary, shell, &[]);
    }

    fn socket_root(temporary: &TempDir) -> PathBuf {
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure tmux test root");
        // tmux never unlinks its socket, so a server in the shared socket directory leaks a
        // file there on every test run.
        let tmux_tmpdir = temporary.path().join("tmux-sockets");
        fs::create_dir_all(&tmux_tmpdir).expect("create isolated tmux socket root");
        fs::set_permissions(&tmux_tmpdir, Permissions::from_mode(0o700))
            .expect("secure isolated tmux socket root");

        return tmux_tmpdir;
    }

    fn new_with_config_args(temporary: &TempDir, shell: &str, config_arguments: &[&str]) -> Self {
        let binary = tmux_binary();
        let tmux_tmpdir = Self::socket_root(temporary);

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
            .env("TMUX_TMPDIR", &tmux_tmpdir)
            .env("SHELL", shell)
            .status()
            .expect("start private tmux server");

        assert!(status.success(), "private tmux server did not start");

        return Self {
            binary,
            socket,
            session: "recorded".to_owned(),
            tmux_tmpdir,
        };
    }

    fn unmanaged(temporary: &TempDir, shell: &str) -> Self {
        let binary = tmux_binary();
        let tmux_tmpdir = Self::socket_root(temporary);

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
            .env("TMUX_TMPDIR", &tmux_tmpdir)
            .env("SHELL", shell)
            .status()
            .expect("start unmanaged private tmux server");

        assert!(status.success(), "private tmux server did not start");

        return Self {
            binary,
            socket,
            session: "existing".to_owned(),
            tmux_tmpdir,
        };
    }

    fn unmanaged_default(temporary: &TempDir, shell: &str) -> Self {
        let binary = tmux_binary();
        let tmux_tmpdir = Self::socket_root(temporary);

        let output = StdCommand::new(&binary)
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
            .env_remove("TMUX")
            .env("SHELL", shell)
            .output()
            .expect("start isolated default tmux server");

        assert!(
            output.status.success(),
            "isolated default tmux server did not start: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        return Self {
            binary,
            socket: "default".to_owned(),
            session: "existing".to_owned(),
            tmux_tmpdir,
        };
    }

    fn command(&self) -> StdCommand {
        let mut command = StdCommand::new(&self.binary);
        command.args(["-L", self.socket.as_str()]);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);

        return command;
    }

    fn termtrace(&self) -> Command {
        return termtrace_in(&self.tmux_tmpdir);
    }

    fn socket_path(&self) -> ByteString {
        let output = self
            .command()
            .args(["display-message", "-p", "#{socket_path}"])
            .output()
            .expect("query tmux socket path");

        assert!(output.status.success(), "tmux socket path was not reported");

        return ByteString::from(trim_newline(output.stdout));
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

    fn wait_for_status(
        &self,
        description: &str,
        predicate: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            let output = self
                .termtrace()
                .args(["tmux", "status", "--json", "--tmux"])
                .arg(&self.binary)
                .args(["-L", self.socket.as_str()])
                .output()
                .expect("query tmux status");

            assert!(
                output.status.success(),
                "tmux status failed while waiting for {description}: {}",
                String::from_utf8_lossy(&output.stderr)
            );

            let status = compact_json_line(&output.stdout);
            if predicate(&status) {
                return status;
            }

            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}: last status={status}"
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
            let status = self
                .termtrace()
                .args(["tmux", "status", "--json", "--tmux"])
                .arg(&self.binary)
                .args(["-L", self.socket.as_str()])
                .output()
                .expect("diagnose observers");

            panic!(
                "unrecorded pane {pane} remained alive; panes={}; status={}",
                String::from_utf8_lossy(&panes.stdout),
                String::from_utf8_lossy(&status.stdout),
            );
        }
    }
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "tmux test helpers stop immediately when runtime evidence violates the test contract"
)]
fn wait_for_recorded_resize(session: &Path, cols: u16, rows: u16) {
    let events = session.join("events.jsonl");
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        let diagnostic = match EventLog::read(&events) {
            Ok(log) => {
                let observed = log
                    .events
                    .iter()
                    .filter_map(|record| match &record.event {
                        Event::ResizeIntent { cols, rows, .. } => return Some((*cols, *rows)),
                        _ => return None,
                    })
                    .collect::<Vec<_>>();

                if observed.contains(&(cols, rows)) {
                    return;
                }

                format!("observed resize intents={observed:?}")
            }
            Err(error) => format!("event log was not readable: {error}"),
        };

        assert!(
            Instant::now() < deadline,
            "recording {} did not observe resize {cols}x{rows}: {diagnostic}",
            session.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when the isolated server cannot be inspected"
)]
fn wait_for_tmux_server_exit(server_pid: u64) {
    let server_pid = server_pid.to_string();
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        let output = StdCommand::new("kill")
            .args(["-0", server_pid.as_str()])
            .output()
            .expect("probe original tmux server PID");
        if !output.status.success() {
            return;
        }

        assert!(
            Instant::now() < deadline,
            "original tmux server process {server_pid} remained alive after kill-server"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        // Drop has no error channel; the private server may already have exited.
        drop(
            self.command()
                .arg("kill-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        );
    }
}

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when the Termtrace binary cannot be built"
)]
fn termtrace_in(tmux_tmpdir: &Path) -> Command {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    command.env("TMUX_TMPDIR", tmux_tmpdir);

    return command;
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

fn tmux_binary() -> PathBuf {
    let binary = if Path::new("/usr/bin/tmux").is_file() {
        PathBuf::from("/usr/bin/tmux")
    } else {
        PathBuf::from("tmux")
    };

    let status = StdCommand::new(&binary)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    assert!(
        status.as_ref().is_ok_and(|status| return status.success()),
        "tmux is required by the CLI integration suite: {status:?}"
    );

    return binary;
}
