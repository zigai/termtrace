use super::{
    capture_io::{check_cancelled, external_error},
    media::{validate_screenshot_probe, validate_video_probe},
    process::{
        ghostty_overrides, probe_output_with_check, spawn_xvfb, wait_while_running, DisplayLease,
        ManagedChild,
    },
    Result,
};
use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    result::Result as StdResult,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};
use termtrace_evidence::artifact::{create_private_directory, write_secure};
use termtrace_model::execution::session::WindowPadding;
use termtrace_model::protocol::capability::{
    Capability, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_SCREENSHOT_TOOLS,
    NATIVE_VIDEO_CAPABILITY, NATIVE_VIDEO_TOOLS,
};
use termtrace_process::find_executable;

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
const WIDTH: u16 = 640;
const HEIGHT: u16 = 400;
const FPS: u16 = 10;
const PROBE: &str = "native readiness probe";

type ProbeResult = StdResult<(), String>;

/// Probe native dependencies and perform bounded private-display capture in stable report order.
///
/// Setting `cancelled` stops the capture probe, which then tears down its display, terminal,
/// and staging directory and reports the cancellation as a probe failure.
pub fn native_capabilities(
    screenshot: bool,
    video: bool,
    cancelled: &AtomicBool,
) -> impl Iterator<Item = Capability> + use<> {
    return native_capabilities_with(
        screenshot,
        video,
        |program| return find_executable(program).is_some(),
        |screenshot, video| return operational_probe(screenshot, video, cancelled),
    )
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

    let screenshot_missing = screenshot.then(|| return missing(NATIVE_SCREENSHOT_TOOLS));
    let video_missing = video.then(|| return missing(NATIVE_VIDEO_TOOLS));

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
            if video { "native_video_dependency_missing" } else { "native_host_tools_missing" },
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
            if video { "native_video_ready" } else { "native_screenshot_ready" },
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

fn operational_probe(screenshot: bool, video: bool, cancelled: &AtomicBool) -> [ProbeResult; 2] {
    let staging = env::temp_dir().join(format!("termtrace-capability-{}", uuid::Uuid::now_v7()));
    if let Err(error) = create_private_directory(&staging) {
        return failed_probes(error.into());
    }

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let result = probe_staged(&staging, deadline, screenshot, video, cancelled);
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
    cancelled: &AtomicBool,
) -> Result<[ProbeResult; 2]> {
    let probe = Probe {
        deadline,
        cancelled,
    };

    probe.check()?;

    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(None, &lease, WIDTH, HEIGHT, staging, None)?;
    let socket = lease.socket();
    probe.wait_until(&mut xvfb, probe.remaining()?, || return socket.exists())?;

    let ready = staging.join("fixture-ready");
    // The fixture is a shell builtin-only program, not the caller's executable: library hosts
    // need not implement any CLI, and no descendant sleeps survive the terminal's teardown.
    let mut command = Command::new("dbus-run-session");
    command
        .args(["--", "ghostty"])
        .args(ghostty_overrides(WindowPadding::default()))
        .args(["--config-default-files=false", "--background=102030", "--foreground=ffffff"])
        .args(["-e", "/bin/sh", "-c", r#"printf '\033[2J\033[Htermtrace native readiness\n'; printf ready > "$1"; read -r answer"#, "termtrace-native-probe"])
        .arg(&ready)
        .env("DISPLAY", &display)
        .env("XDG_CONFIG_HOME", staging)
        .env("GDK_BACKEND", "x11")
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdin(Stdio::null());
    let mut ghostty = ManagedChild::spawn_group("ghostty", &mut command)?;
    // A first Ghostty launch on a fresh machine (GTK, fonts, software GL) can outlast one
    // step, so it may use the rest of the probe deadline.
    probe.wait_until(&mut ghostty, probe.until_deadline()?, || {
        return ready.is_file();
    })?;
    ghostty.check_running()?;

    let screenshot_result = if screenshot {
        probe_capture(staging, &display, &probe, false).map_err(|error| return error.to_string())
    } else {
        Ok(())
    };
    ghostty.check_running()?;
    xvfb.check_running()?;

    let video_result = if video {
        probe_capture(staging, &display, &probe, true).map_err(|error| return error.to_string())
    } else {
        Ok(())
    };
    ghostty.check_running()?;
    xvfb.check_running()?;

    ghostty.terminate();
    xvfb.terminate();

    return Ok([screenshot_result, video_result]);
}

struct Probe<'cancel> {
    deadline: Instant,
    cancelled: &'cancel AtomicBool,
}

impl Probe<'_> {
    fn check(&self) -> Result<()> {
        return check_cancelled(self.cancelled, PROBE);
    }

    fn until_deadline(&self) -> Result<Duration> {
        return self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| return external_error(PROBE, "20 second deadline expired"));
    }

    fn remaining(&self) -> Result<Duration> {
        return self
            .until_deadline()
            .map(|duration| return duration.min(STEP_TIMEOUT));
    }

    fn wait_until(
        &self,
        process: &mut ManagedChild,
        timeout: Duration,
        ready: impl Fn() -> bool,
    ) -> Result<()> {
        return wait_while_running(
            process,
            timeout,
            &format!("timed out after {} ms", timeout.as_millis()),
            || {
                self.check()?;

                return Ok(ready().then_some(()));
            },
        );
    }

    fn run(&self, name: &'static str, command: &mut Command) -> Result<Vec<u8>> {
        return probe_output_with_check(name, command, self.remaining()?, || return self.check());
    }
}

fn wait_for_painted_frame(display: &str, probe: &Probe<'_>, video: bool) -> Result<()> {
    let deadline = probe.deadline.min(Instant::now() + STEP_TIMEOUT);

    loop {
        let sample = if video {
            probe.run(
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
            )?
        } else {
            probe.run(
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

fn probe_capture(staging: &Path, display: &str, probe: &Probe<'_>, video: bool) -> Result<()> {
    wait_for_painted_frame(display, probe, video)?;

    let path = staging.join(if video { "probe.mkv" } else { "probe.png" });
    let bytes = if video {
        probe.run(
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
        )?
    } else {
        probe.run(
            "import",
            Command::new("import")
                .args(["-silent", "-window", "root", "png:-"])
                .env("DISPLAY", display),
        )?
    };

    write_secure(&path, &bytes)?;

    let name = if video { "video-ffprobe" } else { "screenshot-ffprobe" };
    let bytes = probe.run(
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
