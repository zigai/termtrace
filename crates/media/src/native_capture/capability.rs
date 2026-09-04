use std::process::{Command, Stdio};

use termtrace_model::protocol::capability::{
    Capability, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY,
};

/// Probe native screenshot and video dependencies in stable report order.
#[must_use]
pub fn native_capabilities() -> [Capability; 2] {
    return native_capabilities_with(available, ffmpeg_has_encoder);
}

fn native_capabilities_with(
    available_fn: impl Fn(&str) -> bool,
    has_encoder_fn: impl Fn(&str) -> bool,
) -> [Capability; 2] {
    let missing = |programs: &[&str]| -> Vec<String> {
        programs
            .iter()
            .filter(|program| !available_fn(program))
            .map(|program| (*program).to_owned())
            .collect()
    };
    let screenshot_missing = missing(&["ghostty", "Xvfb", "dbus-run-session", "import", "ffprobe"]);
    let video_missing = missing(&["ghostty", "Xvfb", "dbus-run-session", "ffmpeg", "ffprobe"]);
    let encoder = video_missing.is_empty() && has_encoder_fn("libx264rgb");
    return [
        capability(
            NATIVE_SCREENSHOT_CAPABILITY,
            "native_screenshot_tools_ready",
            "native_host_tools_missing",
            &screenshot_missing,
            true,
        ),
        capability(
            NATIVE_VIDEO_CAPABILITY,
            "native_video_tools_ready",
            "native_video_dependency_missing",
            &video_missing,
            encoder,
        ),
    ];
}

fn capability(
    id: &str,
    ready_code: &str,
    blocked_code: &str,
    missing: &[String],
    extra_ready: bool,
) -> Capability {
    if cfg!(target_os = "linux") && missing.is_empty() && extra_ready {
        return Capability {
            id: id.to_owned(),
            status: CapabilityStatus::Ready,
            code: ready_code.to_owned(),
            detail: "private-display native Ghostty capture dependencies are available".to_owned(),
            remediation: None,
        };
    }
    let mut detail = if cfg!(target_os = "linux") {
        format!("missing tools: {}", missing.join(", "))
    } else {
        "native capture requires Linux".to_owned()
    };
    if id == NATIVE_VIDEO_CAPABILITY && missing.is_empty() && !extra_ready {
        detail = "FFmpeg lacks the required libx264rgb encoder".to_owned();
    }
    return Capability {
        id: id.to_owned(),
        status: CapabilityStatus::Blocked,
        code: blocked_code.to_owned(),
        detail,
        remediation: Some(
            "install the listed native tools; native capture has no fallback".to_owned(),
        ),
    };
}

fn available(program: &str) -> bool {
    return Command::new("sh")
        .args([
            "-c",
            "command -v \"$1\" >/dev/null 2>&1",
            "termtrace-probe",
            program,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| return status.success());
}
fn ffmpeg_has_encoder(encoder: &str) -> bool {
    return Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| {
            return output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .any(|value| return value == encoder);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn native_video_does_not_require_screenshot_only_import() {
        let [screenshot, video] = native_capabilities_with(|prog| prog != "import", |_| true);
        assert_eq!(screenshot.status, CapabilityStatus::Blocked);
        assert!(screenshot.detail.contains("import"));
        assert_eq!(video.status, CapabilityStatus::Ready);
    }
}
