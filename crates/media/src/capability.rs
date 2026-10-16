//! Bounded capability probes owned by the media layer.

use crate::native_capture::process::probe_output;
use std::{process::Command, result::Result as StdResult, time::Duration};
use termtrace_model::protocol::capability::{
    Capability, CapabilityStatus, PRESENTATION_CAPABILITY, PRESENTATION_TOOLS,
    SEMANTIC_VIDEO_CAPABILITY, SEMANTIC_VIDEO_TOOLS,
};
use termtrace_process::find_executable;

const DEPENDENCY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Probe deterministic semantic-video encoding without creating an artifact.
///
/// `semantic_ready` is the result of the semantic-engine probe, which this capability needs.
#[must_use]
pub fn semantic_video_capability(semantic_ready: bool) -> Capability {
    let missing = missing(SEMANTIC_VIDEO_TOOLS);
    if !missing.is_empty() {
        return blocked(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_dependency_missing",
            format!("missing tools: {}", missing.join(", ")),
            "install FFmpeg/FFprobe with libx264rgb",
        );
    }

    if !semantic_ready {
        return blocked(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_engine_blocked",
            "the semantic engine is blocked",
            "reinstall a build containing the pinned semantic engine",
        );
    }
    return match probe_semantic_video_encode() {
        Ok(()) => ready(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_tools_ready",
            "the semantic engine and a bounded libx264rgb encode probe completed; native display tools are not required",
        ),
        Err(error) => blocked(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_probe_failed",
            format!("the libx264rgb encode probe failed: {error}"),
            "install an FFmpeg build with the libx264rgb encoder",
        ),
    };
}

/// Probe presentation delivery encoding and subtitle-filter support.
#[must_use]
pub fn presentation_capability() -> Capability {
    let missing = missing(PRESENTATION_TOOLS);
    if !missing.is_empty() {
        return blocked(
            PRESENTATION_CAPABILITY,
            "presentation_dependency_missing",
            format!("missing tools: {}", missing.join(", ")),
            "install FFmpeg/FFprobe with libx264 and libass support",
        );
    }

    let encoder = ffmpeg_has_entry("-encoders", "libx264");
    let ass_filter = ffmpeg_has_entry("-filters", "ass");

    if encoder && ass_filter {
        return ready(
            PRESENTATION_CAPABILITY,
            "presentation_tools_ready",
            "FFmpeg/FFprobe, H.264 delivery encoding, and the ASS/libass filter were found",
        );
    }

    let absent: Vec<&str> = [
        (encoder, "libx264 encoder"),
        (ass_filter, "ASS/libass filter"),
    ]
    .into_iter()
    .filter(|(available, _)| return !available)
    .map(|(_, feature)| return feature)
    .collect();

    return blocked(
        PRESENTATION_CAPABILITY,
        "presentation_ffmpeg_unsupported",
        format!("FFmpeg lacks: {}", absent.join(", ")),
        "install an FFmpeg build with libx264 and libass support",
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
        .filter(|command| return find_executable(command).is_none())
        .map(|command| return (*command).to_owned())
        .collect();
}

fn ffmpeg_has_entry(argument: &str, entry: &str) -> bool {
    let Some(ffmpeg) = find_executable("ffmpeg") else {
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
    let ffmpeg =
        find_executable("ffmpeg").ok_or_else(|| return "FFmpeg is unavailable".to_owned())?;

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
