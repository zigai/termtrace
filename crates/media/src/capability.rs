//! Bounded capability probes owned by the media layer.

use std::{
    env, ffi::OsStr, os::unix::fs::PermissionsExt as _, path::PathBuf, process::Command,
    result::Result as StdResult, time::Duration,
};

use termtrace_model::protocol::capability::{
    Capability, CapabilityStatus, PRESENTATION_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
};
use termtrace_terminal::GhosttyVt;

use crate::native_capture::process::probe_output;

const DEPENDENCY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Probe semantic-video and presentation dependencies in stable report order.
#[must_use]
pub fn derived_media_capabilities() -> [Capability; 2] {
    return [semantic_video_capability(), presentation_capability()];
}

/// Probe deterministic semantic-video encoding without creating an artifact.
#[must_use]
pub fn semantic_video_capability() -> Capability {
    let semantic_ready = GhosttyVt::new(2, 2, 0)
        .and_then(|mut terminal| return terminal.snapshot())
        .is_ok();
    let missing = missing(&["ffmpeg", "ffprobe"]);
    let encode_probe = if missing.is_empty() {
        probe_semantic_video_encode()
    } else {
        Err("probe was not run because FFmpeg or FFprobe is missing".to_owned())
    };
    if semantic_ready && encode_probe.is_ok() {
        return ready(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_tools_ready",
            "the semantic engine and a bounded libx264rgb encode probe completed; native display tools are not required",
        );
    }
    return blocked(
        SEMANTIC_VIDEO_CAPABILITY,
        "semantic_video_dependency_missing",
        format!(
            "semantic engine: {}; missing tools: {}; encode probe: {}",
            if semantic_ready { "ready" } else { "blocked" },
            missing.join(", "),
            encode_probe
                .as_ref()
                .map_or_else(|error| return error.as_str(), |()| return "ready"),
        ),
        "install FFmpeg/FFprobe with libx264rgb and use a build containing the pinned semantic engine",
    );
}

/// Probe presentation delivery encoding and subtitle-filter support.
#[must_use]
pub fn presentation_capability() -> Capability {
    let missing = missing(&["ffmpeg", "ffprobe"]);
    let encoder = missing.is_empty() && ffmpeg_has_entry("-encoders", "libx264");
    let ass_filter = encoder && ffmpeg_has_entry("-filters", "ass");
    if missing.is_empty() && encoder && ass_filter {
        return ready(
            PRESENTATION_CAPABILITY,
            "presentation_tools_ready",
            "FFmpeg/FFprobe, H.264 delivery encoding, and the ASS/libass filter were found",
        );
    }
    return blocked(
        PRESENTATION_CAPABILITY,
        "presentation_dependency_missing",
        format!(
            "missing tools: {}; libx264 encoder: {}; ASS/libass filter: {}",
            missing.join(", "),
            availability(encoder),
            availability(ass_filter),
        ),
        "install FFmpeg/FFprobe with libx264 and libass support",
    );
}

fn ready(id: &str, code: &str, detail: impl Into<String>) -> Capability {
    return Capability {
        id: id.to_owned(),
        status: CapabilityStatus::Ready,
        code: code.to_owned(),
        detail: detail.into(),
        remediation: None,
    };
}

fn blocked(
    id: &str,
    code: &str,
    detail: impl Into<String>,
    remediation: impl Into<String>,
) -> Capability {
    return Capability {
        id: id.to_owned(),
        status: CapabilityStatus::Blocked,
        code: code.to_owned(),
        detail: detail.into(),
        remediation: Some(remediation.into()),
    };
}

fn missing(commands: &[&str]) -> Vec<String> {
    return commands
        .iter()
        .filter(|command| return executable(command).is_none())
        .map(|command| return (*command).to_owned())
        .collect();
}

fn executable(command: impl AsRef<OsStr>) -> Option<PathBuf> {
    let command = command.as_ref();
    let path = env::var_os("PATH")?;
    return env::split_paths(&path)
        .map(|directory| return directory.join(command))
        .find(|candidate| {
            return candidate.metadata().is_ok_and(|metadata| {
                return metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
            });
        });
}

fn ffmpeg_has_entry(argument: &str, entry: &str) -> bool {
    let Some(ffmpeg) = executable("ffmpeg") else {
        return false;
    };
    return probe_output(
        "ffmpeg",
        Command::new(ffmpeg).args(["-hide_banner", argument]),
        DEPENDENCY_PROBE_TIMEOUT,
    )
    .is_ok_and(|output| {
        return String::from_utf8_lossy(&output)
            .split_whitespace()
            .any(|value| return value == entry);
    });
}

pub(crate) fn probe_semantic_video_encode() -> StdResult<(), String> {
    let ffmpeg = executable("ffmpeg").ok_or_else(|| return "FFmpeg is unavailable".to_owned())?;
    probe_output(
        "ffprobe",
        Command::new("ffprobe").arg("-version"),
        DEPENDENCY_PROBE_TIMEOUT,
    )
    .map_err(|error| return error.to_string())?;
    return probe_output(
        "ffmpeg",
        Command::new(ffmpeg).args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=16x16:d=0.04",
            "-frames:v",
            "1",
            "-vf",
            "format=rgb24",
            "-c:v",
            "libx264rgb",
            "-crf",
            "0",
            "-preset",
            "medium",
            "-f",
            "null",
            "-",
        ]),
        DEPENDENCY_PROBE_TIMEOUT,
    )
    .map(|_| return)
    .map_err(|error| return error.to_string());
}

fn availability(value: bool) -> &'static str {
    if value {
        return "available";
    }
    return "unavailable";
}
