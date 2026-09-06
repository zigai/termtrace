use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use termtrace_evidence::artifact::{secure_file, write_secure};
use termtrace_model::artifact::visual::{CaptureOptions, NativeFrameSample};

use super::{
    Error, Result,
    process::{ManagedChild, probe_output, probe_output_with_check, wait_for_success},
    util::{external_error, unix_now_ns},
};

pub(super) fn spawn_video_ffmpeg(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    video: &Path,
) -> Result<ManagedChild> {
    let path = staging.join("ffmpeg-video.log");
    let log = secure_file(&path)?;
    let clone = log
        .try_clone()
        .map_err(|source| return Error::Filesystem { path, source })?;
    let size = format!("{}x{}", options.width, options.height);
    let fps = options.fps.to_string();
    return ManagedChild::spawn_logged(
        "ffmpeg",
        Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "info",
                "-copyts",
                "-f",
                "x11grab",
                "-framerate",
                &fps,
                "-draw_mouse",
                "0",
                "-video_size",
                &size,
                "-i",
                display,
                "-an",
                "-vf",
                "showinfo,setpts=PTS-STARTPTS",
                "-c:v",
                "libx264rgb",
                "-threads",
                "1",
                "-crf",
                "0",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "rgb24",
                "-fps_mode",
                "passthrough",
                "-y",
            ])
            .arg(video)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(clone))
            .stderr(Stdio::from(log)),
    );
}

pub(super) fn capture_screenshot(
    options: &CaptureOptions,
    staging: &Path,
    display: &str,
    screenshot: &Path,
) -> Result<()> {
    let size = format!("{}x{}", options.width, options.height);
    let log = secure_file(&staging.join("ffmpeg-screenshot.log"))?;
    let mut child = ManagedChild::spawn_logged(
        "ffmpeg screenshot",
        Command::new("ffmpeg")
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log)),
    )?;
    return wait_for_success(&mut child, Duration::from_secs(15), staging);
}

pub(super) fn capture_stable_x11_frame(
    staging: &Path,
    display: &str,
    screenshot: &Path,
) -> Result<Vec<NativeFrameSample>> {
    return capture_stable_x11_frame_with_check(
        staging,
        display,
        screenshot,
        Duration::from_secs(30),
        || return Ok(()),
    );
}

pub(super) fn capture_stable_x11_frame_with_check(
    staging: &Path,
    display: &str,
    screenshot: &Path,
    timeout: Duration,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<NativeFrameSample>> {
    let deadline = Instant::now() + timeout;
    let mut samples: Vec<NativeFrameSample> = Vec::new();
    let mut previous_hash: Option<String> = None;
    let mut stable_run = 0_u8;
    let mut log: Vec<u8> = Vec::new();
    for sample in 0_u16..20 {
        check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(external_error("X11 frame import", "capture timed out"));
        }
        let path = staging.join(format!(".native-sample-{sample:04}.png"));
        let output = probe_output_with_check(
            "X11 frame import",
            Command::new("import")
                .args(["-silent", "-window", "root"])
                .arg(format!("png:{}", path.display()))
                .env("DISPLAY", display),
            remaining,
            &mut check,
        );
        if let Err(error) = output {
            log.extend_from_slice(error.to_string().as_bytes());
            write_secure(&staging.join("x11-screenshot.log"), &log)?;
            return Err(error);
        }
        let bytes = fs::read(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        fs::remove_file(&path).map_err(|source| return Error::Filesystem { path, source })?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let same_as_previous = previous_hash.as_ref() == Some(&hash);
        stable_run = if same_as_previous {
            stable_run.saturating_add(1)
        } else {
            1
        };
        samples.push(NativeFrameSample {
            sample,
            captured_unix_ns: unix_now_ns()?,
            blake3: hash.clone(),
            same_as_previous,
        });
        previous_hash = Some(hash);
        if stable_run >= 3 {
            write_secure(screenshot, &bytes)?;
            write_secure(&staging.join("x11-screenshot.log"), &log)?;
            return Ok(samples);
        }
        thread::sleep(Duration::from_millis(50));
    }
    write_secure(&staging.join("x11-screenshot.log"), &log)?;
    return Err(Error::ExternalCommand {
        operation: "X11 frame import".to_owned(),
        reason: "native frame did not produce three consecutive identical samples".to_owned(),
    });
}

pub(super) fn probe_video(
    path: &Path,
    fps: u16,
    width: u16,
    height: u16,
) -> Result<serde_json::Value> {
    let probe = ffprobe(path, &["-count_packets", "-show_streams", "-show_format"])?;
    validate_video_probe(&probe, fps, width, height)?;
    return Ok(probe);
}

pub(super) fn validate_video_probe(
    probe: &serde_json::Value,
    fps: u16,
    width: u16,
    height: u16,
) -> Result<()> {
    let stream = first_stream(probe)?;
    let rate = format!("{fps}/1");
    let packets = stream
        .get("nb_read_packets")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| return value.parse::<u64>().ok())
        .unwrap_or(0);
    if stream
        .get("avg_frame_rate")
        .and_then(serde_json::Value::as_str)
        != Some(&rate)
        || stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || packets == 0
    {
        return Err(Error::InvalidReplay {
            reason: format!("video probe does not match {width}x{height} at {rate} with packets"),
        });
    }
    return Ok(());
}
pub(super) fn probe_screenshot(path: &Path, width: u16, height: u16) -> Result<serde_json::Value> {
    let probe = ffprobe(path, &["-show_streams"])?;
    validate_screenshot_probe(&probe, width, height)?;
    return Ok(probe);
}

pub(super) fn validate_screenshot_probe(
    probe: &serde_json::Value,
    width: u16,
    height: u16,
) -> Result<()> {
    let stream = first_stream(probe)?;
    if stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || stream.get("codec_name").and_then(serde_json::Value::as_str) != Some("png")
    {
        return Err(Error::InvalidReplay {
            reason: "screenshot dimensions or codec do not match".to_owned(),
        });
    }
    return Ok(());
}
fn ffprobe(path: &Path, args: &[&str]) -> Result<serde_json::Value> {
    let output = probe_output(
        "ffprobe",
        Command::new("ffprobe")
            .args(["-v", "error"])
            .args(args)
            .args(["-of", "json"])
            .arg(path),
        Duration::from_secs(30),
    )?;
    return serde_json::from_slice(&output).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

pub(super) fn probe_variable_video(
    path: &Path,
    width: u16,
    height: u16,
) -> Result<serde_json::Value> {
    let probe = ffprobe(path, &["-count_packets", "-show_streams", "-show_format"])?;
    let stream = first_stream(&probe)?;
    if stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || stream
            .get("nb_read_packets")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| return value.parse::<u64>().ok())
            .unwrap_or(0)
            == 0
    {
        return Err(external_error(
            "Wayland video",
            "encoded stream has incorrect geometry or no frames",
        ));
    }
    return Ok(probe);
}
fn first_stream(probe: &serde_json::Value) -> Result<&serde_json::Value> {
    return probe
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .and_then(|streams| return streams.first())
        .ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "media probe has no stream".to_owned(),
            };
        });
}
