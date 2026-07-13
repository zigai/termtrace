//! Native Ghostty capture on an isolated Xvfb display.

use std::{
    fs::{File, OpenOptions, Permissions},
    io::{BufRead as _, BufReader, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
        sync_directory, write_secure,
    },
    event_log::EventLog,
    playback::PlaybackTiming,
    replay::{DERIVED_DIRECTORY, StateIndexEntry},
};

/// Native visual artifact directory name.
pub const VISUAL_DIRECTORY: &str = "visual";

/// Native capture configuration.
#[derive(Clone, Debug)]
pub struct CaptureOptions {
    /// Source session directory with semantic index.
    pub session_directory: PathBuf,
    /// Xvfb/capture width in pixels.
    pub width: u16,
    /// Xvfb/capture height in pixels.
    pub height: u16,
    /// Encoded video frame rate.
    pub fps: u16,
}

/// Completed native capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOutcome {
    /// Atomically committed visual directory.
    pub directory: PathBuf,
    /// Video path.
    pub video: PathBuf,
    /// Final native screenshot path.
    pub screenshot: PathBuf,
}

/// State-to-video mapping entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VideoMapEntry {
    /// Semantic state identifier.
    pub state_id: u64,
    /// Semantic state hash.
    pub state_hash: String,
    /// Monotonic semantic timestamp.
    pub monotonic_ns: u64,
    /// Approximate zero-based video frame; absent for suppressed states.
    pub frame: Option<u64>,
    /// Whether Ghostty was allowed to present this state.
    pub presented: bool,
}

/// Provenance and probe evidence for native visual artifacts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Source session identifier.
    pub session_id: Uuid,
    /// Terminal hash of the source event log.
    pub source_event_hash: Option<String>,
    /// Video frame rate.
    pub fps: u16,
    /// Capture width.
    pub width: u16,
    /// Capture height.
    pub height: u16,
    /// Wall-clock time used as approximate video frame zero.
    pub capture_started_unix_ns: u128,
    /// Playback event-time origin.
    pub replay_origin_unix_ns: u128,
    /// BLAKE3 of Ghostty's resolved configuration.
    pub ghostty_config_hash: String,
    /// Installed Ghostty version report.
    pub ghostty_version: String,
    /// `FFmpeg` version first line.
    pub ffmpeg_version: String,
    /// Exact headless launch overrides.
    pub ghostty_overrides: Vec<String>,
    /// Parsed `FFprobe` video evidence.
    pub video_probe: serde_json::Value,
    /// Parsed `FFprobe` screenshot evidence.
    pub screenshot_probe: serde_json::Value,
}

struct ManagedChild {
    name: &'static str,
    child: Option<Child>,
}

struct DisplayLease {
    number: u16,
    lock_path: PathBuf,
}

/// Capture timed replay with the actual Ghostty GTK/OpenGL application.
///
/// # Errors
///
/// Returns an error for invalid dimensions, missing semantic index, process
/// launch/exit failures, timeouts, probe mismatches, or artifact I/O failure.
pub fn capture(options: &CaptureOptions) -> Result<CaptureOutcome> {
    validate_options(options)?;
    let event_log = EventLog::read(options.session_directory.join("events.jsonl"))?;
    let state_index = read_state_index(&options.session_directory)?;
    let destination = options.session_directory.join(VISUAL_DIRECTORY);
    if destination.exists() {
        return Err(Error::ArtifactExists { path: destination });
    }
    let staging = options
        .session_directory
        .join(format!(".visual.{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    if let Err(error) = capture_staged(options, &event_log, &state_index, &staging) {
        return Err(preserve_failed_capture(
            &options.session_directory,
            &staging,
            error,
        ));
    }
    let commit = commit_directory(&options.session_directory, &staging, &destination)
        .and_then(|()| sync_directory(&options.session_directory));
    if let Err(error) = commit {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| Error::Filesystem {
            path: staging,
            source,
        });
        return finish_with_cleanup(Err(error), cleanup);
    }
    Ok(outcome(destination))
}

fn preserve_failed_capture(session: &Path, staging: &Path, error: Error) -> Error {
    let evidence = session.join(format!("visual-failed-{}", Uuid::now_v7()));
    match commit_directory(session, staging, &evidence).and_then(|()| sync_directory(session)) {
        Ok(()) => Error::CaptureFailed {
            evidence,
            source: Box::new(error),
        },
        Err(cleanup) => Error::CleanupFailed {
            primary: Box::new(error),
            cleanup: Box::new(cleanup),
        },
    }
}

fn capture_staged(
    options: &CaptureOptions,
    event_log: &EventLog,
    state_index: &[StateIndexEntry],
    staging: &Path,
) -> Result<VisualManifest> {
    let resolved_config = command_output("ghostty", &["+show-config"])?;
    write_secure(&staging.join("ghostty-config.txt"), &resolved_config)?;
    let ghostty_version =
        String::from_utf8_lossy(&command_output("ghostty", &["+version"])?).into();
    let ffmpeg_version_output = command_output("ffmpeg", &["-version"])?;
    let ffmpeg_version = String::from_utf8_lossy(&ffmpeg_version_output)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();

    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(&lease, options, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let timing_file = staging.join("playback-timing.json");
    let ready_file = staging.join("playback-ready");
    let overrides = ghostty_overrides();
    let mut ghostty = spawn_ghostty(
        options,
        staging,
        &display,
        &timing_file,
        &ready_file,
        &overrides,
    )?;
    let timing: PlaybackTiming =
        wait_for_json(&timing_file, &mut ghostty, Duration::from_secs(20))?;
    thread::sleep(Duration::from_millis(200));

    let capture_started_unix_ns = unix_now_ns()?;
    let video_path = staging.join("session.mkv");
    let mut ffmpeg = spawn_video_ffmpeg(options, staging, &display, &video_path)?;
    let recording_duration = event_log.events.last().map_or(Duration::ZERO, |event| {
        Duration::from_nanos(event.monotonic_ns)
    });
    wait_for_file(
        &ready_file,
        &mut ghostty,
        Duration::from_secs(25) + recording_duration,
    )?;
    let screenshot_path = staging.join("final.png");
    capture_screenshot(options, staging, &display, &screenshot_path)?;
    wait_for_success(&mut ghostty, Duration::from_secs(10), staging)?;
    stop_ffmpeg(&mut ffmpeg, Duration::from_secs(10), staging)?;
    xvfb.terminate();

    secure_permissions(&video_path)?;
    secure_permissions(&screenshot_path)?;
    let video_probe = probe_video(&video_path, options.fps, options.width, options.height)?;
    let screenshot_probe = probe_screenshot(&screenshot_path, options.width, options.height)?;
    write_video_map(
        &staging.join("video-map.jsonl"),
        state_index,
        options.fps,
        capture_started_unix_ns,
        timing.replay_origin_unix_ns,
    )?;
    let manifest = VisualManifest {
        version: 1,
        session_id: event_log.header.session_id,
        source_event_hash: event_log.events.last().map(|event| event.hash.clone()),
        fps: options.fps,
        width: options.width,
        height: options.height,
        capture_started_unix_ns,
        replay_origin_unix_ns: timing.replay_origin_unix_ns,
        ghostty_config_hash: blake3::hash(&resolved_config).to_hex().to_string(),
        ghostty_version,
        ffmpeg_version,
        ghostty_overrides: overrides,
        video_probe,
        screenshot_probe,
    };
    let encoded = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| json_error(staging.join("manifest.json"), source))?;
    write_secure(&staging.join("manifest.json"), &encoded)?;
    for transient in [&timing_file, &ready_file] {
        fs_err::remove_file(transient).map_err(|source| Error::Filesystem {
            path: transient.clone(),
            source,
        })?;
    }
    sync_directory(staging)?;
    drop(lease);
    Ok(manifest)
}

fn validate_options(options: &CaptureOptions) -> Result<()> {
    if options.width < 320 || options.height < 200 || options.fps != 24 {
        return Err(Error::InvalidReplay {
            reason: "native capture requires at least 320x200 pixels and exactly 24 FPS".to_owned(),
        });
    }
    if !options
        .session_directory
        .join(DERIVED_DIRECTORY)
        .join("state-index.jsonl")
        .is_file()
    {
        return Err(Error::InvalidReplay {
            reason: "semantic index is missing; run `termtrace index` first".to_owned(),
        });
    }
    Ok(())
}

fn ghostty_overrides() -> Vec<String> {
    vec![
        "--gtk-single-instance=false".to_owned(),
        "--confirm-close-surface=false".to_owned(),
        "--quit-after-last-window-closed=true".to_owned(),
        "--window-position-x=0".to_owned(),
        "--window-position-y=0".to_owned(),
        "--window-width=9999".to_owned(),
        "--window-height=9999".to_owned(),
        "--window-save-state=never".to_owned(),
    ]
}

fn spawn_xvfb(
    lease: &DisplayLease,
    options: &CaptureOptions,
    staging: &Path,
) -> Result<ManagedChild> {
    let log = secure_file(&staging.join("xvfb.log"))?;
    let screen = format!("{}x{}x24", options.width, options.height);
    let child = Command::new("Xvfb")
        .arg(format!(":{}", lease.number))
        .args(["-screen", "0", &screen, "-nolisten", "tcp", "-noreset"])
        .stdout(Stdio::from(log.try_clone().map_err(|source| {
            Error::Filesystem {
                path: staging.join("xvfb.log"),
                source,
            }
        })?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| external_error("Xvfb", &error))?;
    Ok(ManagedChild::new("Xvfb", child))
}

fn spawn_ghostty(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    timing_file: &Path,
    ready_file: &Path,
    overrides: &[String],
) -> Result<ManagedChild> {
    let stdout = secure_file(&staging.join("ghostty.stdout.log"))?;
    let stderr = secure_file(&staging.join("ghostty.stderr.log"))?;
    let executable = std::env::current_exe().map_err(|source| Error::Filesystem {
        path: PathBuf::from("<current-executable>"),
        source,
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
        .arg("--ready-file")
        .arg(ready_file)
        .arg("--start-delay-ms")
        .arg("1500")
        .arg("--final-hold-ms")
        .arg("1500")
        .env("DISPLAY", display)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command
        .spawn()
        .map_err(|error| external_error("ghostty", &error))?;
    Ok(ManagedChild::new("ghostty", child))
}

fn spawn_video_ffmpeg(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    video: &Path,
) -> Result<ManagedChild> {
    let log = secure_file(&staging.join("ffmpeg-video.log"))?;
    let size = format!("{}x{}", options.width, options.height);
    let child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "info",
            "-f",
            "x11grab",
            "-framerate",
            &options.fps.to_string(),
            "-draw_mouse",
            "0",
            "-video_size",
            &size,
            "-i",
            display,
            "-an",
            "-c:v",
            "libx264rgb",
            "-crf",
            "0",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "rgb24",
            "-fps_mode",
            "cfr",
            "-y",
        ])
        .arg(video)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log.try_clone().map_err(|source| {
            Error::Filesystem {
                path: staging.join("ffmpeg-video.log"),
                source,
            }
        })?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| external_error("ffmpeg", &error))?;
    Ok(ManagedChild::new("ffmpeg", child))
}

fn capture_screenshot(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    screenshot: &Path,
) -> Result<()> {
    let size = format!("{}x{}", options.width, options.height);
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "x11grab",
            "-draw_mouse",
            "0",
            "-video_size",
            &size,
            "-i",
            display,
            "-frames:v",
            "1",
            "-y",
        ])
        .arg(screenshot)
        .output()
        .map_err(|error| external_error("ffmpeg screenshot", &error))?;
    if !output.status.success() {
        write_secure(&staging.join("ffmpeg-screenshot.log"), &output.stderr)?;
        return Err(status_error("ffmpeg screenshot", output.status));
    }
    write_secure(&staging.join("ffmpeg-screenshot.log"), &output.stderr)
}

fn probe_video(path: &Path, fps: u16, width: u16, height: u16) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|error| external_error("ffprobe", &error))?;
    if !output.status.success() {
        return Err(status_error("ffprobe", output.status));
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|source| json_error(path.to_path_buf(), source))?;
    let expected = format!("{fps}/1");
    let stream = probe["streams"]
        .as_array()
        .and_then(|streams| streams.first())
        .ok_or_else(|| invalid_probe("video has no stream"))?;
    let frame_count = stream["nb_read_frames"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if stream["avg_frame_rate"].as_str() != Some(&expected)
        || stream["width"].as_u64() != Some(u64::from(width))
        || stream["height"].as_u64() != Some(u64::from(height))
        || frame_count == 0
    {
        return Err(invalid_probe(
            "video frame-rate or frame-count probe failed",
        ));
    }
    Ok(probe)
}

fn probe_screenshot(path: &Path, width: u16, height: u16) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|error| external_error("ffprobe screenshot", &error))?;
    if !output.status.success() {
        return Err(status_error("ffprobe screenshot", output.status));
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|source| json_error(path.to_path_buf(), source))?;
    let stream = probe["streams"]
        .as_array()
        .and_then(|streams| streams.first())
        .ok_or_else(|| invalid_probe("screenshot has no stream"))?;
    if stream["width"].as_u64() != Some(u64::from(width))
        || stream["height"].as_u64() != Some(u64::from(height))
        || stream["codec_name"].as_str() != Some("png")
    {
        return Err(invalid_probe("screenshot dimensions or codec do not match"));
    }
    Ok(probe)
}

fn read_state_index(session: &Path) -> Result<Vec<StateIndexEntry>> {
    let path = session.join(DERIVED_DIRECTORY).join("state-index.jsonl");
    let reader = BufReader::new(File::open(&path).map_err(|source| Error::Filesystem {
        path: path.clone(),
        source,
    })?);
    reader
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line.map_err(|source| Error::Filesystem {
                path: path.clone(),
                source,
            })?;
            serde_json::from_str(&line).map_err(|source| Error::InvalidJson {
                path: path.clone(),
                json_path: format!("line {}", index + 1),
                source,
            })
        })
        .collect()
}

fn write_video_map(
    path: &Path,
    states: &[StateIndexEntry],
    fps: u16,
    capture_origin: u128,
    replay_origin: u128,
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    for state in states {
        let presentation = replay_origin.saturating_add(u128::from(state.monotonic_ns));
        let relative = presentation.saturating_sub(capture_origin);
        let frame = state.presented.then(|| {
            u64::try_from(relative.saturating_mul(u128::from(fps)) / 1_000_000_000)
                .unwrap_or(u64::MAX)
        });
        let entry = VideoMapEntry {
            state_id: state.state_id,
            state_hash: state.state_hash.clone(),
            monotonic_ns: state.monotonic_ns,
            frame,
            presented: state.presented,
        };
        serde_json::to_writer(&mut writer, &entry)
            .map_err(|source| json_error(path.to_path_buf(), source))?;
        writer
            .write_all(b"\n")
            .map_err(|source| Error::Filesystem {
                path: path.to_path_buf(),
                source,
            })?;
    }
    finish_writer(writer, path)
}

impl DisplayLease {
    fn acquire() -> Result<Self> {
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
                    writeln!(file, "{}", std::process::id()).map_err(|source| {
                        Error::Filesystem {
                            path: lock_path.clone(),
                            source,
                        }
                    })?;
                    return Ok(Self { number, lock_path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(Error::Filesystem {
                        path: lock_path,
                        source,
                    });
                }
            }
        }
        Err(Error::ExternalCommand {
            program: "Xvfb".to_owned(),
            reason: "no private display number is available".to_owned(),
        })
    }
}

impl Drop for DisplayLease {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.lock_path);
    }
}

impl ManagedChild {
    fn new(name: &'static str, child: Child) -> Self {
        Self {
            name,
            child: Some(child),
        }
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .as_mut()
            .expect("managed child is available")
            .try_wait()
            .map_err(|error| external_error(self.name, &error))
    }

    fn terminate(&mut self) {
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

fn wait_for_x_socket(
    lease: &DisplayLease,
    process: &mut ManagedChild,
    timeout: Duration,
) -> Result<()> {
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{}", lease.number));
    wait_until(process, timeout, || socket.exists())
}

fn wait_for_file(path: &Path, process: &mut ManagedChild, timeout: Duration) -> Result<()> {
    wait_until(process, timeout, || path.is_file())
}

fn wait_for_json<T: for<'de> Deserialize<'de>>(
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

fn wait_for_success(process: &mut ManagedChild, timeout: Duration, staging: &Path) -> Result<()> {
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

fn stop_ffmpeg(process: &mut ManagedChild, timeout: Duration, staging: &Path) -> Result<()> {
    process
        .child
        .as_mut()
        .expect("managed child is available")
        .stdin
        .as_mut()
        .ok_or_else(|| Error::ExternalCommand {
            program: process.name.to_owned(),
            reason: "standard-input control pipe is unavailable".to_owned(),
        })?
        .write_all(b"q\n")
        .map_err(|error| external_error(process.name, &error))?;
    wait_for_success(process, timeout, staging)
}

fn secure_permissions(path: &Path) -> Result<()> {
    fs_err::set_permissions(path, Permissions::from_mode(0o600)).map_err(|source| {
        Error::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn command_output(program: &str, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| external_error(program, &error))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(status_error(program, output.status))
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs_err::read(path).map_err(|source| Error::Filesystem {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| json_error(path.to_path_buf(), source))
}

fn unix_now_ns() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|_| Error::ClockBeforeEpoch)
}

fn outcome(directory: PathBuf) -> CaptureOutcome {
    CaptureOutcome {
        video: directory.join("session.mkv"),
        screenshot: directory.join("final.png"),
        directory,
    }
}

fn status_error(program: &str, status: ExitStatus) -> Error {
    Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!("exit status {status}"),
    }
}

fn status_error_with_logs(program: &str, status: ExitStatus, staging: &Path) -> Error {
    Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!(
            "exit status {status}; inspect logs in {}",
            staging.display()
        ),
    }
}

fn external_error(program: &str, error: &impl ToString) -> Error {
    Error::ExternalCommand {
        program: program.to_owned(),
        reason: error.to_string(),
    }
}

fn invalid_probe(reason: &str) -> Error {
    Error::ExternalCommand {
        program: "ffprobe".to_owned(),
        reason: reason.to_owned(),
    }
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    }
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(Error::CleanupFailed {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}
