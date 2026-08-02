use std::{
    env::current_exe,
    fs::OpenOptions,
    io::{ErrorKind, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use super::{CaptureKind, CaptureOptions};
use crate::{
    Error, Result,
    artifact::secure_file,
    playback::{PlaybackFailure, PlaybackTiming},
};

use super::util::{external_error, read_json, status_error, status_error_with_logs};

pub(super) struct ManagedChild {
    name: &'static str,
    child: Option<Child>,
}

pub(super) struct DisplayLease {
    pub(super) number: u16,
    lock_path: PathBuf,
}

pub(super) fn ghostty_overrides() -> Vec<String> {
    return vec![
        "--gtk-single-instance=false".to_owned(),
        "--confirm-close-surface=false".to_owned(),
        "--quit-after-last-window-closed=true".to_owned(),
        "--window-position-x=0".to_owned(),
        "--window-position-y=0".to_owned(),
        "--window-width=9999".to_owned(),
        "--window-height=9999".to_owned(),
        "--window-save-state=never".to_owned(),
    ];
}

pub(super) fn spawn_xvfb(
    lease: &DisplayLease,
    width: u16,
    height: u16,
    staging: &Path,
) -> Result<ManagedChild> {
    let log = secure_file(&staging.join("xvfb.log"))?;
    let screen = format!("{width}x{height}x24");
    let child = Command::new("Xvfb")
        .arg(format!(":{}", lease.number))
        .args(["-screen", "0", &screen, "-nolisten", "tcp", "-noreset"])
        .stdout(Stdio::from(log.try_clone().map_err(|source| {
            return Error::Filesystem {
                path: staging.join("xvfb.log"),
                source,
            };
        })?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| return external_error("Xvfb", &error))?;
    return Ok(ManagedChild::new("Xvfb", child));
}

pub(super) fn spawn_geometry_probe(
    staging: &Path,
    display: &str,
    report_path: &Path,
    overrides: &[String],
    profile_root: &Path,
    log_stem: &str,
) -> Result<ManagedChild> {
    let stdout_path = staging.join(format!("{log_stem}.stdout.log"));
    let stderr_path = staging.join(format!("{log_stem}.stderr.log"));
    let stdout = secure_file(&stdout_path)?;
    let stderr = secure_file(&stderr_path)?;
    let executable = current_exe().map_err(|source| {
        return Error::Filesystem {
            path: PathBuf::from("<current-executable>"),
            source,
        };
    })?;
    let mut command = Command::new("dbus-run-session");
    command
        .arg("--")
        .arg("ghostty")
        .args(overrides)
        .arg("-e")
        .arg(executable)
        .arg("geometry-probe")
        .arg("--out")
        .arg(report_path)
        .env("DISPLAY", display)
        .env("XDG_CONFIG_HOME", profile_root)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command
        .spawn()
        .map_err(|error| return external_error("ghostty geometry probe", &error))?;
    return Ok(ManagedChild::new("ghostty geometry probe", child));
}

pub(super) fn spawn_ghostty(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    timing_file: &Path,
    error_file: &Path,
    geometry_file: &Path,
    ready_file: &Path,
    overrides: &[String],
    profile_root: Option<&Path>,
) -> Result<ManagedChild> {
    let stdout = secure_file(&staging.join("ghostty.stdout.log"))?;
    let stderr = secure_file(&staging.join("ghostty.stderr.log"))?;
    let executable = current_exe().map_err(|source| {
        return Error::Filesystem {
            path: PathBuf::from("<current-executable>"),
            source,
        };
    })?;
    let mut command = Command::new("dbus-run-session");
    command.arg("--").arg("ghostty").args(overrides).arg("-e");
    command
        .arg(executable)
        .arg("playback")
        .arg("--session")
        .arg(&options.session_directory)
        .arg("--timing-file")
        .arg(timing_file)
        .arg("--error-file")
        .arg(error_file)
        .arg("--geometry-file")
        .arg(geometry_file)
        .arg("--ready-file")
        .arg(ready_file)
        .arg("--start-delay-ms")
        .arg(if matches!(options.kind, CaptureKind::Video) {
            "1500"
        } else {
            "0"
        })
        .arg("--final-hold-ms")
        .arg("1500");
    if let Some(start) = options.start_monotonic_ns {
        command.arg("--from-ns").arg(start.to_string());
    }
    if let Some(end) = options.end_monotonic_ns {
        command.arg("--to-ns").arg(end.to_string());
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
        if let Some(offset) = event_byte_offset {
            command
                .arg("--endpoint-event-byte-offset")
                .arg(offset.to_string());
        }
    }
    command
        .env("DISPLAY", display)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(profile_root) = profile_root {
        command.env("XDG_CONFIG_HOME", profile_root);
    }
    let child = command
        .spawn()
        .map_err(|error| return external_error("ghostty", &error))?;
    return Ok(ManagedChild::new("ghostty", child));
}

impl DisplayLease {
    pub(super) fn acquire() -> Result<Self> {
        for number in 200_u16..400 {
            let lock_path = PathBuf::from(format!("/tmp/termtrace-X{number}.lock"));
            if Path::new(&format!("/tmp/.X{number}-lock")).exists()
                || Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()
            {
                continue;
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", process::id()).map_err(|source| {
                        return Error::Filesystem {
                            path: lock_path.clone(),
                            source,
                        };
                    })?;
                    return Ok(Self { number, lock_path });
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
            program: "Xvfb".to_owned(),
            reason: "no private display number is available".to_owned(),
        });
    }
}

impl Drop for DisplayLease {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.lock_path);
    }
}

impl ManagedChild {
    pub(super) fn new(name: &'static str, child: Child) -> Self {
        return Self {
            name,
            child: Some(child),
        };
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Err(Error::ExternalCommand {
                program: self.name.to_owned(),
                reason: "managed child is unavailable".to_owned(),
            });
        };
        return child
            .try_wait()
            .map_err(|error| return external_error(self.name, &error));
    }

    pub(super) fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
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
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{}", lease.number));
    return wait_until(process, timeout, || return socket.exists());
}

pub(super) fn wait_for_file(
    path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<()> {
    return wait_until(process, timeout, || return path.is_file());
}

pub(super) fn wait_for_json<T: for<'de> Deserialize<'de>>(
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
                program: process.name.to_owned(),
                reason: format!("timed out waiting for JSON handshake `{}`", path.display()),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn wait_for_playback_handshake(
    timing_path: &Path,
    error_path: &Path,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<PlaybackTiming> {
    let deadline = Instant::now() + timeout;
    loop {
        if error_path.is_file()
            && let Ok(failure) = read_json::<PlaybackFailure>(error_path)
        {
            return Err(Error::InvalidReplay {
                reason: format!("playback child failed before capture: {}", failure.message),
            });
        }
        if timing_path.is_file()
            && let Ok(timing) = read_json(timing_path)
        {
            return Ok(timing);
        }
        if let Some(status) = process.try_wait()? {
            return Err(status_error(process.name, status));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                program: process.name.to_owned(),
                reason: format!(
                    "timed out waiting for playback timing `{}` or failure `{}`",
                    timing_path.display(),
                    error_path.display()
                ),
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
            return Err(status_error(process.name, status));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                program: process.name.to_owned(),
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
            process.child.take();
            if status.success() {
                return Ok(());
            }
            return Err(status_error_with_logs(process.name, status, staging));
        }
        if Instant::now() >= deadline {
            return Err(Error::ExternalCommand {
                program: process.name.to_owned(),
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
            program: process.name.to_owned(),
            reason: "managed child is unavailable".to_owned(),
        };
    })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| {
            return Error::ExternalCommand {
                program: process.name.to_owned(),
                reason: "standard-input control pipe is unavailable".to_owned(),
            };
        })?
        .write_all(b"q\n")
        .map_err(|error| return external_error(process.name, &error))?;
    return wait_for_success(process, timeout, staging);
}
