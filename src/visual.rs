//! Native Ghostty capture on an isolated Xvfb display.

use std::{
    env::{current_exe, temp_dir},
    fs::{File, OpenOptions, Permissions},
    io::{BufRead as _, BufReader, ErrorKind, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        commit_directory, create_private_directory, finish_writer, secure_file, secure_writer,
        sync_directory, write_secure,
    },
    event_log::EventLog,
    geometry::TerminalGeometryReport,
    playback::PlaybackTiming,
    profile::{ResolvedGhosttyProfile, materialize_profile},
    replay::{DERIVED_DIRECTORY, StateIndexEntry},
    session::{CaptureTarget, read_manifest},
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

/// Geometry and configuration measured from Ghostty at a target viewport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FittedGeometry {
    /// Live XTWINOPS geometry report.
    pub report: TerminalGeometryReport,
}

/// State-to-video mapping entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
        .and_then(|()| return sync_directory(&options.session_directory));
    if let Err(error) = commit {
        let cleanup = fs_err::remove_dir_all(&staging).map_err(|source| {
            return Error::Filesystem {
                path: staging,
                source,
            };
        });
        return finish_with_cleanup(Err(error), cleanup);
    }
    return Ok(outcome(destination));
}

/// Measure Ghostty's real grid and cell dimensions at a capture viewport.
///
/// # Errors
///
/// Returns an error when Ghostty, Xvfb, D-Bus, terminal queries, or temporary
/// evidence persistence fails.
pub fn probe_ghostty_geometry(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<FittedGeometry> {
    if target.width < 320 || target.height < 200 {
        return Err(Error::InvalidReplay {
            reason: "fitted geometry requires a viewport of at least 320x200 pixels".to_owned(),
        });
    }
    let staging = temp_dir().join(format!("termtrace-geometry-{}", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = probe_ghostty_geometry_staged(target, profile, &staging);
    match result {
        Ok(fitted) => {
            fs_err::remove_dir_all(&staging).map_err(|source| {
                return Error::Filesystem {
                    path: staging,
                    source,
                };
            })?;
            return Ok(fitted);
        }
        Err(source) => {
            return Err(Error::CaptureFailed {
                evidence: staging,
                source: Box::new(source),
            });
        }
    }
}

fn probe_ghostty_geometry_staged(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
    staging: &Path,
) -> Result<FittedGeometry> {
    write_secure(
        &staging.join("ghostty-config.txt"),
        &profile.effective_config,
    )?;
    let profile_root =
        materialize_profile(&staging.join("ghostty-profile"), &profile.effective_config)?;
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(&lease, target.width, target.height, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let report_path = staging.join("geometry.json");
    let overrides = ghostty_overrides();
    let mut ghostty =
        spawn_geometry_probe(staging, &display, &report_path, &overrides, &profile_root)?;
    let report: TerminalGeometryReport =
        wait_for_json(&report_path, &mut ghostty, Duration::from_secs(20))?;
    wait_for_success(&mut ghostty, Duration::from_secs(5), staging)?;
    xvfb.terminate();
    let geometry = report.session_geometry()?;
    if geometry.pixel_width > target.width || geometry.pixel_height > target.height {
        return Err(Error::InvalidReplay {
            reason: format!(
                "Ghostty text area {}x{} exceeds requested viewport {}x{}",
                geometry.pixel_width, geometry.pixel_height, target.width, target.height
            ),
        });
    }
    drop(lease);
    return Ok(FittedGeometry { report });
}

fn preserve_failed_capture(session: &Path, staging: &Path, error: Error) -> Error {
    let evidence = session.join(format!("visual-failed-{}", Uuid::now_v7()));
    match commit_directory(session, staging, &evidence)
        .and_then(|()| return sync_directory(session))
    {
        Ok(()) => {
            return Error::CaptureFailed {
                evidence,
                source: Box::new(error),
            };
        }
        Err(cleanup) => {
            return Error::CleanupFailed {
                primary: Box::new(error),
                cleanup: Box::new(cleanup),
            };
        }
    }
}

fn capture_staged(
    options: &CaptureOptions,
    event_log: &EventLog,
    state_index: &[StateIndexEntry],
    staging: &Path,
) -> Result<VisualManifest> {
    let session_manifest = read_manifest(&options.session_directory)?;
    let profile_root = if let Some(profile) = &session_manifest.ghostty_visual {
        if profile.effective_config != "ghostty-config.txt" {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    profile.effective_config
                ),
            });
        }
        let config_path = options.session_directory.join(&profile.effective_config);
        let config = fs_err::read(&config_path).map_err(|source| {
            return Error::Filesystem {
                path: config_path,
                source,
            };
        })?;
        let actual_hash = blake3::hash(&config).to_hex().to_string();
        if actual_hash != profile.effective_config_hash {
            return Err(Error::InvalidReplay {
                reason: "recorded Ghostty profile hash does not match session provenance"
                    .to_owned(),
            });
        }
        Some(materialize_profile(
            &staging.join("ghostty-profile"),
            &config,
        )?)
    } else {
        None
    };
    let resolved_config =
        command_output_in_profile("ghostty", &["+show-config"], profile_root.as_deref())?;
    let config_hash = blake3::hash(&resolved_config).to_hex().to_string();
    if let Some(expected) = session_manifest
        .geometry_provenance
        .ghostty_config_hash
        .as_deref()
        && expected != config_hash
    {
        return Err(Error::ExternalCommand {
            program: "ghostty".to_owned(),
            reason: format!(
                "effective configuration changed since geometry fitting (recorded {expected}, current {config_hash})"
            ),
        });
    }
    if let Some(expected) = event_log
        .header
        .colors
        .source
        .strip_prefix("ghostty_show_config_blake3:")
        && expected != config_hash
    {
        return Err(Error::ExternalCommand {
            program: "ghostty".to_owned(),
            reason: format!(
                "effective configuration changed since recording (recorded {expected}, current {config_hash})"
            ),
        });
    }
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
    let mut xvfb = spawn_xvfb(&lease, options.width, options.height, staging)?;
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
        profile_root.as_deref(),
    )?;
    let timing: PlaybackTiming =
        wait_for_json(&timing_file, &mut ghostty, Duration::from_secs(20))?;
    thread::sleep(Duration::from_millis(200));

    let capture_started_unix_ns = unix_now_ns()?;
    let video_path = staging.join("session.mkv");
    let mut ffmpeg = spawn_video_ffmpeg(options, staging, &display, &video_path)?;
    let recording_duration = event_log.events.last().map_or(Duration::ZERO, |event| {
        return Duration::from_nanos(event.monotonic_ns);
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
        source_event_hash: event_log
            .events
            .last()
            .map(|event| return event.hash.clone()),
        fps: options.fps,
        width: options.width,
        height: options.height,
        capture_started_unix_ns,
        replay_origin_unix_ns: timing.replay_origin_unix_ns,
        ghostty_config_hash: config_hash,
        ghostty_version,
        ffmpeg_version,
        ghostty_overrides: overrides,
        video_probe,
        screenshot_probe,
    };
    let encoded = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| return json_error(staging.join("manifest.json"), source))?;
    write_secure(&staging.join("manifest.json"), &encoded)?;
    for transient in [&timing_file, &ready_file] {
        fs_err::remove_file(transient).map_err(|source| {
            return Error::Filesystem {
                path: transient.clone(),
                source,
            };
        })?;
    }
    if let Some(profile_root) = profile_root {
        fs_err::remove_dir_all(&profile_root).map_err(|source| {
            return Error::Filesystem {
                path: profile_root,
                source,
            };
        })?;
    }
    sync_directory(staging)?;
    drop(lease);
    return Ok(manifest);
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
    return Ok(());
}

fn ghostty_overrides() -> Vec<String> {
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

fn spawn_xvfb(
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

fn spawn_geometry_probe(
    staging: &Path,
    display: &str,
    report_path: &Path,
    overrides: &[String],
    profile_root: &Path,
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

fn spawn_ghostty(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    timing_file: &Path,
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
    if let Some(profile_root) = profile_root {
        command.env("XDG_CONFIG_HOME", profile_root);
    }
    let child = command
        .spawn()
        .map_err(|error| return external_error("ghostty", &error))?;
    return Ok(ManagedChild::new("ghostty", child));
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
            return Error::Filesystem {
                path: staging.join("ffmpeg-video.log"),
                source,
            };
        })?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| return external_error("ffmpeg", &error))?;
    return Ok(ManagedChild::new("ffmpeg", child));
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
        .map_err(|error| return external_error("ffmpeg screenshot", &error))?;
    if !output.status.success() {
        write_secure(&staging.join("ffmpeg-screenshot.log"), &output.stderr)?;
        return Err(status_error("ffmpeg screenshot", output.status));
    }
    return write_secure(&staging.join("ffmpeg-screenshot.log"), &output.stderr);
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
        .map_err(|error| return external_error("ffprobe", &error))?;
    if !output.status.success() {
        return Err(status_error("ffprobe", output.status));
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|source| return json_error(path.to_path_buf(), source))?;
    let expected = format!("{fps}/1");
    let stream = probe
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .and_then(|streams| return streams.first())
        .ok_or_else(|| return invalid_probe("video has no stream"))?;
    let frame_count = stream
        .get("nb_read_frames")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| return value.parse::<u64>().ok())
        .unwrap_or(0);
    if stream
        .get("avg_frame_rate")
        .and_then(serde_json::Value::as_str)
        != Some(&expected)
        || stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || frame_count == 0
    {
        return Err(invalid_probe(
            "video frame-rate or frame-count probe failed",
        ));
    }
    return Ok(probe);
}

fn probe_screenshot(path: &Path, width: u16, height: u16) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|error| return external_error("ffprobe screenshot", &error))?;
    if !output.status.success() {
        return Err(status_error("ffprobe screenshot", output.status));
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|source| return json_error(path.to_path_buf(), source))?;
    let stream = probe
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .and_then(|streams| return streams.first())
        .ok_or_else(|| return invalid_probe("screenshot has no stream"))?;
    if stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || stream.get("codec_name").and_then(serde_json::Value::as_str) != Some("png")
    {
        return Err(invalid_probe("screenshot dimensions or codec do not match"));
    }
    return Ok(probe);
}

fn read_state_index(session: &Path) -> Result<Vec<StateIndexEntry>> {
    let path = session.join(DERIVED_DIRECTORY).join("state-index.jsonl");
    let reader = BufReader::new(File::open(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?);
    return reader
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line.map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
            return serde_json::from_str(&line).map_err(|source| {
                return Error::InvalidJson {
                    path: path.clone(),
                    json_path: format!("line {}", index + 1),
                    source,
                };
            });
        })
        .collect();
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
            return u64::try_from(relative.saturating_mul(u128::from(fps)) / 1_000_000_000)
                .unwrap_or(u64::MAX);
        });
        let entry = VideoMapEntry {
            state_id: state.state_id,
            state_hash: state.state_hash.clone(),
            monotonic_ns: state.monotonic_ns,
            frame,
            presented: state.presented,
        };
        serde_json::to_writer(&mut writer, &entry)
            .map_err(|source| return json_error(path.to_path_buf(), source))?;
        writer.write_all(b"\n").map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    }
    return finish_writer(writer, path);
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
    fn new(name: &'static str, child: Child) -> Self {
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
    return wait_until(process, timeout, || return socket.exists());
}

fn wait_for_file(path: &Path, process: &mut ManagedChild, timeout: Duration) -> Result<()> {
    return wait_until(process, timeout, || return path.is_file());
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

fn secure_permissions(path: &Path) -> Result<()> {
    return fs_err::set_permissions(path, Permissions::from_mode(0o600)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

fn command_output(program: &str, arguments: &[&str]) -> Result<Vec<u8>> {
    return command_output_in_profile(program, arguments, None);
}

fn command_output_in_profile(
    program: &str,
    arguments: &[&str],
    profile_root: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(profile_root) = profile_root {
        command.env("XDG_CONFIG_HOME", profile_root);
    }
    let output = command
        .output()
        .map_err(|error| return external_error(program, &error))?;
    if output.status.success() {
        return Ok(output.stdout);
    } else {
        return Err(status_error(program, output.status));
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return serde_json::from_slice(&bytes)
        .map_err(|source| return json_error(path.to_path_buf(), source));
}

fn unix_now_ns() -> Result<u128> {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| return duration.as_nanos())
        .map_err(|_| return Error::ClockBeforeEpoch);
}

fn outcome(directory: PathBuf) -> CaptureOutcome {
    return CaptureOutcome {
        video: directory.join("session.mkv"),
        screenshot: directory.join("final.png"),
        directory,
    };
}

fn status_error(program: &str, status: ExitStatus) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!("exit status {status}"),
    };
}

fn status_error_with_logs(program: &str, status: ExitStatus, staging: &Path) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!(
            "exit status {status}; inspect logs in {}",
            staging.display()
        ),
    };
}

fn external_error(program: &str, error: &impl ToString) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: error.to_string(),
    };
}

fn invalid_probe(reason: &str) -> Error {
    return Error::ExternalCommand {
        program: "ffprobe".to_owned(),
        reason: reason.to_owned(),
    };
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Ok(())) => return Err(primary),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}
