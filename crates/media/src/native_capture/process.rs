use super::{
    capture_io::{external_error, read_json, status_error_with_logs},
    Error, Result,
};
use crate::helper_protocol::{flag, geometry_probe, playback};
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use serde::de::DeserializeOwned;
#[cfg(target_os = "linux")]
use std::os::{linux::net::SocketAddrExt as _, unix::net::SocketAddr};
use std::{
    env, fs,
    io::{ErrorKind, Read, Write as _},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{ChildStderr, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use termtrace_evidence::artifact::secure_file;
use termtrace_model::{
    artifact::playback::{PlaybackFailure, PlaybackTiming},
    artifact::visual::{CaptureKind, CaptureOptions},
    execution::session::WindowPadding,
};
use termtrace_process::{owned, poll_until, SupervisedChild};

/// Grace for a managed group after `SIGTERM`. It exceeds [`owned::GROUP_GRACE`] plus one
/// owner poll so a helper-led group can stop its inner group before being killed.
const TERMINATION_GRACE: Duration = Duration::from_millis(250);
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HANDSHAKE_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(super) struct ManagedChild {
    name: &'static str,
    child: Option<SupervisedChild>,
    output: Option<ProbeOutput>,
}

pub(super) struct DisplayLease {
    pub(super) number: u16,
    // An abstract socket name is released by the kernel when its owner exits and leaves
    // no file behind.
    _socket: UnixListener,
}

struct ProbeOutput {
    stdout: ChildStdout,
    stderr: ChildStderr,
    stdout_bytes: Vec<u8>,
    stderr_bytes: Vec<u8>,
    log: Option<fs::File>,
}

impl ProbeOutput {
    fn drain(&mut self, name: &str) -> Result<()> {
        drain_pipe(&mut self.stdout, &mut self.stdout_bytes, 1_048_576, name)?;
        drain_pipe(&mut self.stderr, &mut self.stderr_bytes, 65_536, name)?;

        return Ok(());
    }
}

fn drain_pipe(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize, name: &str) -> Result<()> {
    let mut buffer = [0_u8; 8192];

    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if count > limit.saturating_sub(bytes.len()) {
                    return Err(external_error(
                        name,
                        format!("probe output exceeded {limit} bytes"),
                    ));
                }

                let chunk = buffer.get(..count).ok_or_else(|| {
                    return external_error(name, "pipe returned an invalid byte count");
                })?;
                bytes.extend_from_slice(chunk);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(external_error(name, error)),
        }
    }
}

/// Run a dependency probe with owned descendants, bounded output and an explicit deadline.
pub(crate) fn probe_output(
    name: &'static str,
    command: &mut Command,
    timeout: Duration,
) -> Result<Vec<u8>> {
    return probe_output_with_check(name, command, timeout, || return Ok(()));
}

pub(super) fn probe_output_with_check(
    name: &'static str,
    command: &mut Command,
    timeout: Duration,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    command.stdin(Stdio::null());
    let mut process = ManagedChild::spawn_group(name, command)?;
    let status = poll_until(Instant::now() + timeout, PROBE_POLL_INTERVAL, || {
        check()?;

        return process.try_wait();
    })?
    .ok_or_else(|| return external_error(name, "timed out while waiting for probe exit"))?;

    if !status.success() {
        return Err(process.failure(status));
    }

    let output = process
        .output
        .take()
        .ok_or_else(|| return external_error(name, "probe output pipes are unavailable"))?;

    return Ok(output.stdout_bytes);
}

pub(super) fn ghostty_overrides(padding: WindowPadding) -> Vec<String> {
    let mut overrides: Vec<String> = [
        "--gtk-single-instance=false",
        "--async-backend=epoll",
        "--confirm-close-surface=false",
        "--quit-after-last-window-closed=true",
        "--window-position-x=0",
        "--window-position-y=0",
        "--window-width=9999",
        "--window-height=9999",
        "--window-save-state=never",
        "--resize-overlay=never",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    if !padding.is_none() {
        overrides.extend([
            format!("--window-padding-x={}", padding.x),
            format!("--window-padding-y={}", padding.y),
            "--window-padding-balance=true".to_owned(),
        ]);
    }

    return overrides;
}

pub(super) fn spawn_xvfb(
    helper: Option<&Path>,
    lease: &DisplayLease,
    width: u16,
    height: u16,
    staging: &Path,
    dpi: Option<u32>,
) -> Result<ManagedChild> {
    let log = secure_file(&staging.join("xvfb.log"))?;
    let mut command = if let Some(helper) = helper {
        owned_command(helper, "Xvfb")?
    } else {
        Command::new("Xvfb")
    };

    let effective_dpi = dpi.or_else(|| {
        return env::var("TERMTRACE_DPI")
            .ok()
            .and_then(|v| return v.parse::<u32>().ok());
    });

    let mut args = vec![
        "-screen".to_owned(),
        "0".to_owned(),
        format!("{width}x{height}x24"),
    ];

    if let Some(dpi) = effective_dpi {
        args.push("-dpi".to_owned());
        args.push(dpi.to_string());
    }

    args.extend([
        "-nolisten".to_owned(),
        "tcp".to_owned(),
        "-noreset".to_owned(),
        "-extension".to_owned(),
        "GLX".to_owned(),
    ]);
    let mut process = ManagedChild::spawn_group(
        "Xvfb",
        command
            .arg(format!(":{}", lease.number))
            .args(&args)
            .stdin(Stdio::null()),
    )?;

    if let Some(output) = process.output.as_mut() {
        output.log = Some(log);
    }

    return Ok(process);
}

pub(super) fn spawn_geometry_probe(
    helper: &Path,
    staging: &Path,
    display: &str,
    report: &Path,
    overrides: &[String],
    profile: &Path,
    scaling: Option<super::capture::DisplayScaling>,
    stem: &str,
) -> Result<ManagedChild> {
    let stdout = secure_file(&staging.join(format!("{stem}.stdout.log")))?;
    let stderr = secure_file(&staging.join(format!("{stem}.stderr.log")))?;
    let mut command = owned_command(helper, "dbus-run-session")?;
    command
        .arg("--")
        .arg("ghostty")
        .args(overrides)
        .arg("-e")
        .arg(helper)
        .arg(geometry_probe::SUBCOMMAND)
        .arg(flag(geometry_probe::OUT))
        .arg(report)
        .env("DISPLAY", display)
        .env("XDG_CONFIG_HOME", profile)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1");

    if let Some(scaling) = scaling {
        command
            .env("GDK_SCALE", scaling.gdk_scale.to_string())
            .env("GDK_DPI_SCALE", format!("{}", scaling.gdk_dpi_scale));
    } else {
        if let Ok(scale) = env::var("GDK_SCALE") {
            command.env("GDK_SCALE", scale);
        }

        if let Ok(dpi_scale) = env::var("GDK_DPI_SCALE") {
            command.env("GDK_DPI_SCALE", dpi_scale);
        }
    }

    return ManagedChild::spawn_logged(
        "ghostty geometry probe",
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr)),
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "the arguments mirror the explicit playback subprocess protocol"
)]
pub(super) fn spawn_ghostty(
    helper: &Path,
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    timing: &Path,
    error: &Path,
    geometry: &Path,
    ready: &Path,
    overrides: &[String],
    profile: Option<&Path>,
    scaling: Option<super::capture::DisplayScaling>,
) -> Result<ManagedChild> {
    let stdout = secure_file(&staging.join("ghostty.stdout.log"))?;
    let stderr = secure_file(&staging.join("ghostty.stderr.log"))?;
    let mut command = owned_command(helper, "dbus-run-session")?;
    command
        .arg("--")
        .arg("ghostty")
        .args(overrides)
        .arg("-e")
        .arg(helper)
        .arg(playback::SUBCOMMAND)
        .arg(flag(playback::SESSION))
        .arg(&options.session_directory)
        .arg(flag(playback::TIMING_FILE))
        .arg(timing)
        .arg(flag(playback::ERROR_FILE))
        .arg(error)
        .arg(flag(playback::GEOMETRY_FILE))
        .arg(geometry)
        .arg(flag(playback::READY_FILE))
        .arg(ready)
        .arg(flag(playback::START_DELAY_MS))
        .arg(if matches!(&options.kind, CaptureKind::Video) { "1500" } else { "0" })
        .arg(flag(playback::FINAL_HOLD_MS))
        .arg("1500");

    if let Some(value) = options.start_monotonic_ns {
        command.arg(flag(playback::FROM_NS)).arg(value.to_string());
    }

    if let Some(value) = options.end_monotonic_ns {
        command.arg(flag(playback::TO_NS)).arg(value.to_string());
    }

    if let CaptureKind::Screenshot {
        event_seq,
        event_byte_offset,
        ..
    } = &options.kind
    {
        command
            .arg(flag(playback::ENDPOINT_EVENT_SEQ))
            .arg(event_seq.to_string())
            .arg(flag(playback::INSTANT));

        if let Some(value) = event_byte_offset {
            command
                .arg(flag(playback::ENDPOINT_EVENT_BYTE_OFFSET))
                .arg(value.to_string());
        }
    }

    command
        .env("DISPLAY", display)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1");

    if let Some(scaling) = scaling {
        command
            .env("GDK_SCALE", scaling.gdk_scale.to_string())
            .env("GDK_DPI_SCALE", format!("{}", scaling.gdk_dpi_scale));
    } else {
        if let Ok(scale) = env::var("GDK_SCALE") {
            command.env("GDK_SCALE", scale);
        }

        if let Ok(dpi_scale) = env::var("GDK_DPI_SCALE") {
            command.env("GDK_DPI_SCALE", dpi_scale);
        }
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    if let Some(profile) = profile {
        command.env("XDG_CONFIG_HOME", profile);
    }

    return ManagedChild::spawn_logged("ghostty", &mut command);
}

/// Launch the explicit helper supervisor without interpolating executable arguments.
pub(super) fn owned_command(helper: &Path, program: &str) -> Result<Command> {
    return owned::command(helper, program)
        .map_err(|error| return external_error("native process identity", error));
}

impl DisplayLease {
    pub(super) fn acquire() -> Result<Self> {
        let in_use = |number: u16| {
            return Path::new(&format!("/tmp/.X{number}-lock")).exists()
                || Path::new(&format!("/tmp/.X11-unix/X{number}")).exists();
        };

        for number in 200_u16..400 {
            if in_use(number) {
                continue;
            }

            let Some(socket) = bind_display_lease(number)? else {
                continue;
            };

            if in_use(number) {
                continue;
            }
            return Ok(Self {
                number,
                _socket: socket,
            });
        }

        return Err(Error::ExternalCommand {
            operation: "Xvfb".to_owned(),
            reason: "no private display number is available".to_owned(),
        });
    }

    pub(super) fn socket(&self) -> PathBuf {
        return PathBuf::from(format!("/tmp/.X11-unix/X{}", self.number));
    }
}

#[cfg(target_os = "linux")]
fn bind_display_lease(number: u16) -> Result<Option<UnixListener>> {
    let name = format!("termtrace-X{number}");
    let address = SocketAddr::from_abstract_name(name.as_bytes())
        .map_err(|error| return external_error("display lease", error))?;

    return match UnixListener::bind_addr(&address) {
        Ok(socket) => Ok(Some(socket)),
        Err(error) if error.kind() == ErrorKind::AddrInUse => Ok(None),
        Err(error) => Err(external_error("display lease", error)),
    };
}

#[cfg(not(target_os = "linux"))]
fn bind_display_lease(_number: u16) -> Result<Option<UnixListener>> {
    return Err(external_error(
        "display lease",
        "native capture requires Linux",
    ));
}

impl ManagedChild {
    pub(super) fn id(&self) -> Option<u32> {
        return self.child.as_ref().map(SupervisedChild::id);
    }

    /// Own a process group while preserving the caller's file-backed output streams.
    pub(super) fn spawn_logged(name: &'static str, command: &mut Command) -> Result<Self> {
        let child =
            SupervisedChild::spawn(command).map_err(|error| return external_error(name, error))?;

        return Ok(Self {
            name,
            child: Some(child),
            output: None,
        });
    }

    pub(super) fn spawn_group(name: &'static str, command: &mut Command) -> Result<Self> {
        let mut managed =
            Self::spawn_logged(name, command.stdout(Stdio::piped()).stderr(Stdio::piped()))?;
        let child = managed
            .child
            .as_mut()
            .ok_or_else(|| return external_error(name, "managed child is unavailable"))?
            .child_mut();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| return external_error(name, "probe stdout pipe is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| return external_error(name, "probe stderr pipe is unavailable"))?;
        fcntl(&stdout, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
            .map_err(|error| return external_error(name, error))?;
        fcntl(&stderr, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
            .map_err(|error| return external_error(name, error))?;

        managed.output = Some(ProbeOutput {
            stdout,
            stderr,
            stdout_bytes: Vec::new(),
            stderr_bytes: Vec::new(),
            log: None,
        });

        return Ok(managed);
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        if let Some(output) = self.output.as_mut() {
            output.drain(self.name)?;
        }

        let status = self
            .child
            .as_mut()
            .ok_or_else(|| {
                return Error::ExternalCommand {
                    operation: self.name.to_owned(),
                    reason: "managed child is unavailable".to_owned(),
                };
            })?
            .try_wait()
            .map_err(|error| return external_error(self.name, error))?;

        if status.is_some()
            && let Some(output) = self.output.as_mut()
        {
            output.drain(self.name)?;
        }

        return Ok(status);
    }

    fn failure(&self, status: ExitStatus) -> Error {
        let detail = self.output.as_ref().map_or_else(String::new, |output| {
            return String::from_utf8_lossy(&output.stderr_bytes)
                .chars()
                .take(2048)
                .collect();
        });

        let detail = detail.trim();
        if detail.is_empty() {
            return external_error(self.name, format!("exited with {status}"));
        }
        return external_error(self.name, format!("exited with {status}: {detail}"));
    }

    pub(super) fn check_running(&mut self) -> Result<()> {
        if let Some(status) = self.try_wait()? {
            return Err(self.failure(status));
        }
        return Ok(());
    }

    pub(super) fn terminate(&mut self) {
        if let Some(child) = self.child.take() {
            let _termination = child.terminate(TERMINATION_GRACE);
        }

        // Termination is teardown without an error channel; retaining output is best-effort.
        if let Some(output) = self.output.as_mut() {
            drop(output.drain(self.name));

            if let Some(mut log) = output.log.take() {
                drop(log.write_all(&output.stdout_bytes));
                drop(log.write_all(&output.stderr_bytes));
            }
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(super) fn wait_for_x_socket(
    lease: &DisplayLease,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<()> {
    let socket = lease.socket();

    return wait_while_running(process, timeout, &timed_out_after(timeout), || {
        return Ok(socket.exists().then_some(()));
    });
}

pub(super) fn wait_for_file(
    path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<()> {
    return wait_while_running(process, timeout, &timed_out_after(timeout), || {
        return Ok(path.is_file().then_some(()));
    });
}

pub(super) fn wait_for_json<T: DeserializeOwned>(
    path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<T> {
    return wait_while_running(
        process,
        timeout,
        &format!("timed out waiting for JSON handshake `{}`", path.display()),
        || return Ok(read_json(path).ok()),
    );
}

pub(super) fn wait_for_playback_handshake(
    timing: &Path,
    failure: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<PlaybackTiming> {
    return wait_while_running(
        process,
        timeout,
        "timed out waiting for playback handshake",
        || {
            if let Ok(value) = read_json::<PlaybackFailure>(failure) {
                return Err(Error::InvalidReplay {
                    reason: format!("playback child failed before capture: {}", value.message),
                    source: None,
                });
            }
            return Ok(read_json(timing).ok());
        },
    );
}

/// Poll `ready` until it yields a value; the process exiting first or the deadline fails.
pub(super) fn wait_while_running<T>(
    process: &mut ManagedChild,
    timeout: Duration,
    timed_out: &str,
    mut ready: impl FnMut() -> Result<Option<T>>,
) -> Result<T> {
    let value = poll_until(Instant::now() + timeout, HANDSHAKE_POLL_INTERVAL, || {
        if let Some(value) = ready()? {
            return Ok(Some(value));
        }

        if let Some(status) = process.try_wait()? {
            return Err(process.failure(status));
        }
        return Ok(None);
    })?;

    return value.ok_or_else(|| return external_error(process.name, timed_out));
}

fn timed_out_after(timeout: Duration) -> String {
    return format!("timed out after {} ms", timeout.as_millis());
}

pub(super) fn wait_for_success(
    process: &mut ManagedChild,
    timeout: Duration,
    staging: &Path,
) -> Result<()> {
    let status = poll_until(Instant::now() + timeout, HANDSHAKE_POLL_INTERVAL, || {
        return process.try_wait();
    })?
    .ok_or_else(|| return external_error(process.name, "timed out while waiting for clean exit"))?;

    process.terminate();

    if !status.success() {
        return Err(status_error_with_logs(process.name, status, staging));
    }
    return Ok(());
}

pub(super) fn stop_ffmpeg(
    process: &mut ManagedChild,
    timeout: Duration,
    staging: &Path,
) -> Result<()> {
    let child = process.child.as_mut().ok_or_else(|| {
        return Error::ExternalCommand {
            operation: process.name.to_owned(),
            reason: "managed child is unavailable".to_owned(),
        };
    })?;
    let stdin = child.child_mut().stdin.as_mut().ok_or_else(|| {
        return Error::ExternalCommand {
            operation: process.name.to_owned(),
            reason: "standard-input control pipe is unavailable".to_owned(),
        };
    })?;
    if let Err(error) = stdin.write_all(b"q\n")
        && error.kind() != ErrorKind::BrokenPipe
    {
        return Err(external_error(process.name, error));
    }
    return wait_for_success(process, timeout, staging);
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::thread;

    #[test]
    fn completed_launcher_cannot_leave_a_background_writer() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker = directory.path().join("survived");
        let mut child = ManagedChild::spawn_logged(
            "test launcher",
            Command::new("sh")
                .args([
                    "-c",
                    "(sleep 0.5; printf survived > \"$1\") & exit 0",
                    "test",
                ])
                .arg(&marker)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("launch process group");
        wait_for_success(&mut child, Duration::from_secs(2), directory.path())
            .expect("launcher exits successfully");

        thread::sleep(Duration::from_millis(600));

        assert!(
            !marker.exists(),
            "background child survived launcher cleanup"
        );
    }

    #[test]
    fn logged_process_timeout_reaps_the_owned_child() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut child = ManagedChild::spawn_logged(
            "test stalled capture",
            Command::new("sh")
                .args(["-c", "exec sleep 60"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("launch stalled child");

        let started = Instant::now();
        let result = wait_for_success(&mut child, Duration::from_millis(30), directory.path());
        assert!(result.is_err());

        child.terminate();

        assert!(child.child.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stop_ffmpeg_ignores_broken_pipe_and_captures_exit_status() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut child = ManagedChild::spawn_logged(
            "test ffmpeg exit",
            Command::new("sh")
                .args(["-c", "exit 42"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("launch process");

        thread::sleep(Duration::from_millis(50));

        let result = stop_ffmpeg(&mut child, Duration::from_secs(2), directory.path());

        match result {
            Err(Error::ExternalCommand { operation, reason }) => {
                assert_eq!(operation, "test ffmpeg exit");
                assert!(
                    reason.contains("42"),
                    "expected reason to contain exit code 42, got: {reason}"
                );
            }
            other => panic!("expected ExternalCommand error with exit code, got {other:?}"),
        }
    }

    #[test]
    fn stop_ffmpeg_ignores_broken_pipe_on_clean_exit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut child = ManagedChild::spawn_logged(
            "test ffmpeg clean",
            Command::new("sh")
                .args(["-c", "exit 0"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("launch process");

        thread::sleep(Duration::from_millis(50));

        let result = stop_ffmpeg(&mut child, Duration::from_secs(2), directory.path());
        assert!(
            result.is_ok(),
            "expected clean exit to succeed, got {result:?}"
        );
    }
}
