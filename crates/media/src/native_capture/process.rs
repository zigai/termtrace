use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{self, Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    fcntl::{FcntlArg, Flock, FlockArg, OFlag, fcntl},
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde::de::DeserializeOwned;
use termtrace_evidence::artifact::secure_file;
use termtrace_model::{
    artifact::playback::{PlaybackFailure, PlaybackTiming},
    artifact::visual::{CaptureKind, CaptureOptions},
};

use super::{
    Error, Result,
    util::{external_error, read_json, status_error_with_logs},
};

pub(super) struct ManagedChild {
    name: &'static str,
    child: Option<Child>,
    group: Option<Pid>,
    output: Option<ProbeOutput>,
}
pub(super) struct DisplayLease {
    pub(super) number: u16,
    // Persistent inode with a kernel lock; owner death releases the lease.
    _lock: Flock<fs::File>,
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
    let deadline = Instant::now() + timeout;
    loop {
        check()?;
        if let Some(status) = process.try_wait()? {
            if !status.success() {
                return Err(process.failure(status));
            }
            let output = process
                .output
                .take()
                .ok_or_else(|| return external_error(name, "probe output pipes are unavailable"))?;
            return Ok(output.stdout_bytes);
        }
        if Instant::now() >= deadline {
            return Err(external_error(
                name,
                "timed out while waiting for probe exit",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn ghostty_overrides() -> Vec<String> {
    return vec![
        "--gtk-single-instance=false",
        "--confirm-close-surface=false",
        "--quit-after-last-window-closed=true",
        "--window-position-x=0",
        "--window-position-y=0",
        "--window-width=9999",
        "--window-height=9999",
        "--window-save-state=never",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
}

pub(super) fn spawn_xvfb(
    helper: Option<&Path>,
    lease: &DisplayLease,
    width: u16,
    height: u16,
    staging: &Path,
) -> Result<ManagedChild> {
    let log = secure_file(&staging.join("xvfb.log"))?;
    let mut command = if let Some(helper) = helper {
        owned_command(helper, "Xvfb")?
    } else {
        Command::new("Xvfb")
    };
    let mut process = ManagedChild::spawn_group(
        "Xvfb",
        command
            .arg(format!(":{}", lease.number))
            .args([
                "-screen",
                "0",
                &format!("{width}x{height}x24"),
                "-nolisten",
                "tcp",
                "-noreset",
                "-extension",
                "GLX",
            ])
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
    stem: &str,
) -> Result<ManagedChild> {
    let stdout = secure_file(&staging.join(format!("{stem}.stdout.log")))?;
    let stderr = secure_file(&staging.join(format!("{stem}.stderr.log")))?;
    return ManagedChild::spawn_logged(
        "ghostty geometry probe",
        owned_command(helper, "dbus-run-session")?
            .arg("--")
            .arg("ghostty")
            .args(overrides)
            .arg("-e")
            .arg(helper)
            .arg("geometry-probe")
            .arg("--out")
            .arg(report)
            .env("DISPLAY", display)
            .env("XDG_CONFIG_HOME", profile)
            .env("GDK_BACKEND", "x11")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
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
        .arg("playback")
        .arg("--session")
        .arg(&options.session_directory)
        .arg("--timing-file")
        .arg(timing)
        .arg("--error-file")
        .arg(error)
        .arg("--geometry-file")
        .arg(geometry)
        .arg("--ready-file")
        .arg(ready)
        .arg("--start-delay-ms")
        .arg(if matches!(&options.kind, CaptureKind::Video) {
            "1500"
        } else {
            "0"
        })
        .arg("--final-hold-ms")
        .arg("1500");
    if let Some(value) = options.start_monotonic_ns {
        command.arg("--from-ns").arg(value.to_string());
    }
    if let Some(value) = options.end_monotonic_ns {
        command.arg("--to-ns").arg(value.to_string());
    }
    if let CaptureKind::Screenshot {
        event_seq,
        event_byte_offset,
        ..
    } = &options.kind
    {
        command
            .arg("--endpoint-event-seq")
            .arg(event_seq.to_string())
            .arg("--instant");
        if let Some(value) = event_byte_offset {
            command
                .arg("--endpoint-event-byte-offset")
                .arg(value.to_string());
        }
    }
    command
        .env("DISPLAY", display)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
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
    let parent = process::id();
    let identity = fs::read_to_string(format!("/proc/{parent}/stat"))
        .map_err(|error| return external_error("native process identity", error))?;
    let ticks = identity
        .rsplit_once(')')
        .and_then(|(_, fields)| return fields.split_whitespace().nth(19))
        .ok_or_else(|| {
            return external_error("native process identity", "missing process start ticks");
        })?;
    let mut command = Command::new(helper);
    command.args([
        "owned-process",
        "--parent-pid",
        &parent.to_string(),
        "--parent-start-ticks",
        ticks,
        "--",
        program,
    ]);
    return Ok(command);
}

impl DisplayLease {
    pub(super) fn acquire() -> Result<Self> {
        for number in 200_u16..400 {
            if Path::new(&format!("/tmp/.X{number}-lock")).exists()
                || Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()
            {
                continue;
            }
            let lock_path = PathBuf::from(format!("/tmp/termtrace-X{number}.lock"));
            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(OFlag::O_NOFOLLOW.bits())
                .mode(0o600)
                .open(&lock_path)
            {
                Ok(file) => {
                    let metadata = file.metadata().map_err(|source| {
                        return Error::Filesystem {
                            path: lock_path.clone(),
                            source,
                        };
                    })?;
                    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                        return Err(external_error(
                            "native display lease",
                            "lease must be a private regular file",
                        ));
                    }
                    let mut file = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                        Ok(lock) => lock,
                        Err((_, Errno::EWOULDBLOCK)) => continue,
                        Err((_, error)) => {
                            return Err(external_error("native display lease", error));
                        }
                    };
                    if Path::new(&format!("/tmp/.X{number}-lock")).exists()
                        || Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()
                    {
                        continue;
                    }
                    file.set_len(0).map_err(|source| {
                        return Error::Filesystem {
                            path: lock_path.clone(),
                            source,
                        };
                    })?;
                    writeln!(file, "{}", process::id()).map_err(|source| {
                        return Error::Filesystem {
                            path: lock_path.clone(),
                            source,
                        };
                    })?;
                    return Ok(Self {
                        number,
                        _lock: file,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(Error::Filesystem {
                        path: lock_path,
                        source,
                    });
                }
            }
        }
        return Err(Error::ExternalCommand {
            operation: "Xvfb".to_owned(),
            reason: "no private display number is available".to_owned(),
        });
    }
}

impl ManagedChild {
    pub(super) fn id(&self) -> Option<u32> {
        return self.child.as_ref().map(Child::id);
    }
    /// Own a process group while preserving the caller's file-backed output streams.
    pub(super) fn spawn_logged(name: &'static str, command: &mut Command) -> Result<Self> {
        let child = command
            .process_group(0)
            .spawn()
            .map_err(|error| return external_error(name, error))?;
        let mut managed = Self::new(name, child);
        let child = managed
            .child
            .as_ref()
            .ok_or_else(|| return external_error(name, "managed child is unavailable"))?;
        managed.group = Some(Pid::from_raw(
            i32::try_from(child.id()).map_err(|error| return external_error(name, error))?,
        ));
        return Ok(managed);
    }
    pub(super) fn new(name: &'static str, child: Child) -> Self {
        return Self {
            name,
            child: Some(child),
            group: None,
            output: None,
        };
    }
    pub(super) fn spawn_group(name: &'static str, command: &mut Command) -> Result<Self> {
        let mut child = command
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| return external_error(name, error))?;
        let group = match i32::try_from(child.id()) {
            Ok(pid) => Pid::from_raw(pid),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(external_error(name, error));
            }
        };
        let mut managed = Self {
            name,
            child: Some(child),
            group: Some(group),
            output: None,
        };
        let child = managed
            .child
            .as_mut()
            .ok_or_else(|| return external_error(name, "managed child is unavailable"))?;
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
        return external_error(
            self.name,
            format!("exited with {status}: {}", detail.trim()),
        );
    }

    pub(super) fn check_running(&mut self) -> Result<()> {
        if let Some(status) = self.try_wait()? {
            return Err(self.failure(status));
        }
        return Ok(());
    }
    pub(super) fn terminate(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = killpg(group, Signal::SIGTERM);
            let deadline = Instant::now() + Duration::from_millis(250);
            while Instant::now() < deadline {
                if killpg(group, None).is_err() {
                    break;
                }
                if let Some(child) = self.child.as_mut() {
                    let _ = child.try_wait();
                }
                thread::sleep(Duration::from_millis(10));
            }
            let _ = killpg(group, Signal::SIGKILL);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(output) = self.output.as_mut() {
            let _ = output.drain(self.name);
            if let Some(mut log) = output.log.take() {
                let _ = log.write_all(&output.stdout_bytes);
                let _ = log.write_all(&output.stderr_bytes);
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
    return wait_until(process, timeout, || {
        return Path::new(&format!("/tmp/.X11-unix/X{}", lease.number)).exists();
    });
}
pub(super) fn wait_for_file(
    path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<()> {
    return wait_until(process, timeout, || return path.is_file());
}
pub(super) fn wait_for_json<T: DeserializeOwned>(
    path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.is_file()
            && let Ok(value) = read_json(path)
        {
            return Ok(value);
        }
        if let Some(status) = process.try_wait()? {
            return Err(status_error(process.name, status));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                operation: process.name.to_owned(),
                reason: format!("timed out waiting for JSON handshake `{}`", path.display()),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}
pub(super) fn wait_for_playback_handshake(
    timing: &Path,
    failure: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<PlaybackTiming> {
    let deadline = Instant::now() + timeout;
    loop {
        if failure.is_file()
            && let Ok(value) = read_json::<PlaybackFailure>(failure)
        {
            return Err(Error::InvalidReplay {
                reason: format!("playback child failed before capture: {}", value.message),
            });
        }
        if timing.is_file()
            && let Ok(value) = read_json(timing)
        {
            return Ok(value);
        }
        if let Some(status) = process.try_wait()? {
            return Err(status_error(process.name, status));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                operation: process.name.to_owned(),
                reason: "timed out waiting for playback handshake".to_owned(),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn wait_until(
    process: &mut ManagedChild,
    timeout: Duration,
    mut condition: impl FnMut() -> bool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return Ok(());
        }
        if let Some(status) = process.try_wait()? {
            return Err(process.failure(status));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                operation: process.name.to_owned(),
                reason: format!("timed out after {} ms", timeout.as_millis()),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}
pub(super) fn wait_for_success(
    process: &mut ManagedChild,
    timeout: Duration,
    staging: &Path,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = process.try_wait()? {
            process.terminate();
            return if status.success() {
                Ok(())
            } else {
                Err(status_error_with_logs(process.name, status, staging))
            };
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                operation: process.name.to_owned(),
                reason: "timed out while waiting for clean exit".to_owned(),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
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
    child
        .stdin
        .as_mut()
        .ok_or_else(|| {
            return Error::ExternalCommand {
                operation: process.name.to_owned(),
                reason: "standard-input control pipe is unavailable".to_owned(),
            };
        })?
        .write_all(b"q\n")
        .map_err(|error| return external_error(process.name, error))?;
    return wait_for_success(process, timeout, staging);
}
fn status_error(program: &str, status: ExitStatus) -> Error {
    return Error::ExternalCommand {
        operation: program.to_owned(),
        reason: format!("exited with {status}"),
    };
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

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
        assert!(child.group.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
