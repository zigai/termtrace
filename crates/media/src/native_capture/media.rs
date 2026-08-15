use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use termtrace_evidence::artifact::{secure_file, write_secure};
use termtrace_model::artifact::visual::{CaptureOptions, NativeFrameSample};

use super::{
    Error, Result,
    process::ManagedChild,
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
    let child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "info",
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
            "-r",
            &fps,
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
        .stdout(Stdio::from(clone))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| return external_error("ffmpeg", error))?;
    return Ok(ManagedChild::new("ffmpeg", child));
}

pub(super) fn capture_screenshot(
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
        .map_err(|error| return external_error("ffmpeg screenshot", error))?;
    write_secure(&staging.join("ffmpeg-screenshot.log"), &output.stderr)?;
    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: "ffmpeg screenshot".to_owned(),
            reason: format!("exited with {}", output.status),
        });
    }
    return Ok(());
}

pub(super) fn capture_stable_x11_frame(
    staging: &Path,
    display: &str,
    screenshot: &Path,
) -> Result<Vec<NativeFrameSample>> {
    let mut samples: Vec<NativeFrameSample> = Vec::new();
    let mut previous_hash: Option<String> = None;
    let mut stable_run = 0_u8;
    let mut log: Vec<u8> = Vec::new();
    for sample in 0_u16..20 {
        let path = staging.join(format!(".native-sample-{sample:04}.png"));
        let output = Command::new("import")
            .args(["-silent", "-window", "root"])
            .arg(format!("png:{}", path.display()))
            .env("DISPLAY", display)
            .output()
            .map_err(|error| return external_error("X11 frame import", error))?;
        log.extend_from_slice(&output.stderr);
        if !output.status.success() {
            write_secure(&staging.join("x11-screenshot.log"), &log)?;
            return Err(Error::ExternalCommand {
                operation: "X11 frame import".to_owned(),
                reason: format!("exited with {}", output.status),
            });
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
    let stream = first_stream(&probe)?;
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
    return Ok(probe);
}
pub(super) fn probe_screenshot(path: &Path, width: u16, height: u16) -> Result<serde_json::Value> {
    let probe = ffprobe(path, &["-show_streams"])?;
    let stream = first_stream(&probe)?;
    if stream.get("width").and_then(serde_json::Value::as_u64) != Some(u64::from(width))
        || stream.get("height").and_then(serde_json::Value::as_u64) != Some(u64::from(height))
        || stream.get("codec_name").and_then(serde_json::Value::as_str) != Some("png")
    {
        return Err(Error::InvalidReplay {
            reason: "screenshot dimensions or codec do not match".to_owned(),
        });
    }
    return Ok(probe);
}
fn ffprobe(path: &Path, args: &[&str]) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .args(["-of", "json"])
        .arg(path)
        .output()
        .map_err(|error| return external_error("ffprobe", error))?;
    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: "ffprobe".to_owned(),
            reason: format!(
                "exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    return serde_json::from_slice(&output.stdout).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
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
