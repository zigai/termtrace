use std::{
    env, fs,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::{Command, Stdio},
    result::Result as StdResult,
    thread,
    time::{Duration, Instant},
};

use termtrace_evidence::artifact::{create_private_directory, write_secure};
use termtrace_model::protocol::capability::{
    Capability, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY,
};

use super::{
    Result,
    media::{validate_screenshot_probe, validate_video_probe},
    process::{
        DisplayLease, ManagedChild, ghostty_overrides, probe_output, spawn_xvfb, wait_for_file,
        wait_for_x_socket,
    },
    util::external_error,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
const WIDTH: u16 = 640;
const HEIGHT: u16 = 400;
const FPS: u16 = 10;

type ProbeResult = StdResult<(), String>;

/// Probe native dependencies and perform bounded private-display capture in stable report order.
pub fn native_capabilities(screenshot: bool, video: bool) -> impl Iterator<Item = Capability> {
    return native_capabilities_with(screenshot, video, available, operational_probe)
        .into_iter()
        .flatten();
}

fn native_capabilities_with(
    screenshot: bool,
    video: bool,
    available_fn: impl Fn(&str) -> bool,
    probe: impl FnOnce(bool, bool) -> [ProbeResult; 2],
) -> [Option<Capability>; 2] {
    let missing = |programs: &[&str]| -> Vec<String> {
        return programs
            .iter()
            .filter(|program| return !available_fn(program))
            .map(|program| return (*program).to_owned())
            .collect();
    };
    let screenshot_missing = screenshot
        .then(|| return missing(&["ghostty", "Xvfb", "dbus-run-session", "import", "ffprobe"]));
    let video_missing = video
        .then(|| return missing(&["ghostty", "Xvfb", "dbus-run-session", "ffmpeg", "ffprobe"]));
    let probe_screenshot =
        cfg!(target_os = "linux") && screenshot_missing.as_ref().is_some_and(Vec::is_empty);
    let probe_video =
        cfg!(target_os = "linux") && video_missing.as_ref().is_some_and(Vec::is_empty);
    let [screenshot_result, video_result] = if probe_screenshot || probe_video {
        probe(probe_screenshot, probe_video)
    } else {
        [Ok(()), Ok(())]
    };
    return [
        screenshot_missing.map(|missing| {
            return capability(NATIVE_SCREENSHOT_CAPABILITY, &missing, screenshot_result);
        }),
        video_missing.map(|missing| {
            return capability(NATIVE_VIDEO_CAPABILITY, &missing, video_result);
        }),
    ];
}

fn capability(id: &str, missing: &[String], result: ProbeResult) -> Capability {
    let video = id == NATIVE_VIDEO_CAPABILITY;
    let (status, code, detail, remediation) = if !cfg!(target_os = "linux") || !missing.is_empty() {
        (
            CapabilityStatus::Blocked,
            if video {
                "native_video_dependency_missing"
            } else {
                "native_host_tools_missing"
            },
            if cfg!(target_os = "linux") {
                format!("missing tools: {}", missing.join(", "))
            } else {
                "native capture requires Linux".to_owned()
            },
            Some("install the listed native tools; native capture has no fallback".to_owned()),
        )
    } else if let Err(reason) = result {
        (
            CapabilityStatus::Blocked,
            if video { "native_video_probe_failed" } else { "native_screenshot_probe_failed" },
            format!("dependencies available, but operational probe failed: {reason}"),
            Some("repair the reported private-display Ghostty/capture failure; native capture has no fallback".to_owned()),
        )
    } else {
        (
            CapabilityStatus::Ready,
            if video {
                "native_video_ready"
            } else {
                "native_screenshot_ready"
            },
            if video {
                "private-display Ghostty launch, libx264rgb capture and video probe succeeded"
                    .to_owned()
            } else {
                "private-display Ghostty launch, screenshot capture and PNG probe succeeded"
                    .to_owned()
            },
            None,
        )
    };
    return Capability {
        id: id.to_owned(),
        status,
        code: code.to_owned(),
        detail,
        remediation,
    };
}

fn available(program: &str) -> bool {
    return env::var_os("PATH").is_some_and(|path| {
        return env::split_paths(&path).any(|directory| {
            return fs::metadata(directory.join(program)).is_ok_and(|metadata| {
                return metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
            });
        });
    });
}

fn operational_probe(screenshot: bool, video: bool) -> [ProbeResult; 2] {
    let staging = env::temp_dir().join(format!("termtrace-capability-{}", uuid::Uuid::now_v7()));
    if let Err(error) = create_private_directory(&staging) {
        return failed_probes(error.into());
    }
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let result = probe_staged(&staging, deadline, screenshot, video);
    let cleanup = fs::remove_dir_all(&staging);
    if let Err(error) = cleanup {
        return failed_probes(external_error("native probe cleanup", error));
    }
    return result.unwrap_or_else(failed_probes);
}

fn failed_probes(error: super::Error) -> [ProbeResult; 2] {
    let detail: String = error.to_string().chars().take(2048).collect();
    return [Err(detail.clone()), Err(detail)];
}

fn probe_staged(
    staging: &Path,
    deadline: Instant,
    screenshot: bool,
    video: bool,
) -> Result<[ProbeResult; 2]> {
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(&lease, WIDTH, HEIGHT, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, remaining(deadline)?)?;
    let ready = staging.join("fixture-ready");
    // The fixture is a shell builtin-only program, not the caller's executable: library hosts
    // need not implement any CLI, and no descendant sleeps survive the terminal's teardown.
    let mut command = Command::new("dbus-run-session");
    command
        .args(["--", "ghostty"])
        .args(ghostty_overrides())
        .args(["--config-default-files=false", "--background=102030", "--foreground=ffffff"])
        .args(["-e", "/bin/sh", "-c", r#"printf '\033[2J\033[Htermtrace native readiness\n'; printf ready > "$1"; read -r answer"#, "termtrace-native-probe"])
        .arg(&ready)
        .env("DISPLAY", &display)
        .env("XDG_CONFIG_HOME", staging)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null());
    let mut ghostty = ManagedChild::spawn_group("ghostty", &mut command)?;
    wait_for_file(&ready, &mut ghostty, remaining(deadline)?)?;
    ghostty.check_running()?;
    let screenshot_result = if screenshot {
        probe_capture(staging, &display, deadline, false).map_err(|error| return error.to_string())
    } else {
        Ok(())
    };
    ghostty.check_running()?;
    xvfb.check_running()?;
    let video_result = if video {
        probe_capture(staging, &display, deadline, true).map_err(|error| return error.to_string())
    } else {
        Ok(())
    };
    ghostty.check_running()?;
    xvfb.check_running()?;
    ghostty.terminate();
    xvfb.terminate();
    return Ok([screenshot_result, video_result]);
}

fn remaining(deadline: Instant) -> Result<Duration> {
    return deadline
        .checked_duration_since(Instant::now())
        .map(|duration| return duration.min(STEP_TIMEOUT))
        .ok_or_else(|| {
            return external_error("native readiness probe", "20 second deadline expired");
        });
}

fn run_probe(name: &'static str, command: &mut Command, deadline: Instant) -> Result<Vec<u8>> {
    return probe_output(name, command, remaining(deadline)?);
}

fn wait_for_painted_frame(display: &str, deadline: Instant, video: bool) -> Result<()> {
    let deadline = deadline.min(Instant::now() + STEP_TIMEOUT);
    loop {
        let sample = if video {
            run_probe(
                "ffmpeg frame readiness",
                Command::new("ffmpeg").args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "x11grab",
                    "-video_size",
                    "640x400",
                    "-draw_mouse",
                    "0",
                    "-i",
                    display,
                    "-frames:v",
                    "1",
                    "-vf",
                    "crop=1:1:320:200,format=rgb24",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ]),
                deadline,
            )?
        } else {
            run_probe(
                "import frame readiness",
                Command::new("import")
                    .args([
                        "-silent",
                        "-window",
                        "root",
                        "-crop",
                        "1x1+320+200",
                        "-depth",
                        "8",
                        "rgb:-",
                    ])
                    .env("DISPLAY", display),
                deadline,
            )?
        };
        if sample == [0x10, 0x20, 0x30] {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(external_error(
                "Ghostty frame readiness",
                "controlled background was not painted before the deadline",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn probe_capture(staging: &Path, display: &str, deadline: Instant, video: bool) -> Result<()> {
    wait_for_painted_frame(display, deadline, video)?;
    let path = staging.join(if video { "probe.mkv" } else { "probe.png" });
    let bytes = if video {
        run_probe(
            "ffmpeg",
            Command::new("ffmpeg").args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "x11grab",
                "-framerate",
                "10",
                "-draw_mouse",
                "0",
                "-video_size",
                "640x400",
                "-i",
                display,
                "-frames:v",
                "2",
                "-an",
                "-r",
                "10",
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
                "-f",
                "matroska",
                "pipe:1",
            ]),
            deadline,
        )?
    } else {
        run_probe(
            "import",
            Command::new("import")
                .args(["-silent", "-window", "root", "png:-"])
                .env("DISPLAY", display),
            deadline,
        )?
    };
    write_secure(&path, &bytes)?;
    let name = if video {
        "video-ffprobe"
    } else {
        "screenshot-ffprobe"
    };
    let bytes = run_probe(
        name,
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-count_packets",
                "-show_streams",
                "-of",
                "json",
            ])
            .arg(&path),
        deadline,
    )?;
    let report: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| return external_error("ffprobe report", error))?;
    if video {
        validate_video_probe(&report, FPS, WIDTH, HEIGHT)?;
        if report
            .pointer("/streams/0/codec_name")
            .and_then(|value| return value.as_str())
            != Some("h264")
        {
            return Err(external_error("ffprobe video", "expected H.264 codec"));
        }
    } else {
        validate_screenshot_probe(&report, WIDTH, HEIGHT)?;
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn native_video_does_not_require_screenshot_only_import() {
        let [screenshot, video] = native_capabilities_with(
            true,
            true,
            |program| return program != "import",
            |_, _| return [Ok(()), Ok(())],
        );
        let screenshot = screenshot.expect("screenshot was selected");
        let video = video.expect("video was selected");
        assert_eq!(screenshot.status, CapabilityStatus::Blocked);
        assert!(screenshot.detail.contains("import"));
        assert_eq!(video.status, CapabilityStatus::Ready);
    }
}
