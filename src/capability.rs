//! Runtime capability inventory and bounded host probes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{env, ffi::OsStr, path::PathBuf, process::Command};

use crate::{Error, Result, ghostty_vt::GhosttyVt, replay::GHOSTTY_REVISION};

/// Core recording capability identifier.
pub const CORE_CAPABILITY: &str = "core";
/// Pinned semantic-engine capability identifier.
pub const SEMANTIC_CAPABILITY: &str = "semantic";
/// Native exact-state screenshot capability identifier.
pub const NATIVE_SCREENSHOT_CAPABILITY: &str = "native-screenshot";
/// Native video-capture capability identifier.
pub const NATIVE_VIDEO_CAPABILITY: &str = "native-video";
/// Deterministic semantic-video source capability identifier.
pub const SEMANTIC_VIDEO_CAPABILITY: &str = "semantic-video";
/// Edited presentation export capability identifier.
pub const PRESENTATION_CAPABILITY: &str = "presentation";
/// SSH receipt-signing capability identifier.
pub const RECEIPT_SIGNING_CAPABILITY: &str = "receipt-signing";

/// Stable capability identifiers in report order.
pub const CAPABILITY_IDS: &[&str] = &[
    CORE_CAPABILITY,
    SEMANTIC_CAPABILITY,
    NATIVE_SCREENSHOT_CAPABILITY,
    NATIVE_VIDEO_CAPABILITY,
    SEMANTIC_VIDEO_CAPABILITY,
    PRESENTATION_CAPABILITY,
    RECEIPT_SIGNING_CAPABILITY,
];

/// Stable capability readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// The bounded local probe succeeded.
    Ready,
    /// A required component or guarantee is unavailable.
    Blocked,
}

/// Result of one bounded capability probe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    /// Stable capability identifier.
    pub id: String,
    /// Readiness result.
    pub status: CapabilityStatus,
    /// Stable diagnostic code.
    pub code: String,
    /// Human-readable probe result.
    pub detail: String,
    /// Focused remediation when blocked.
    pub remediation: Option<String>,
}

/// Complete local capability report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityReport {
    /// Package version.
    pub termtrace_version: String,
    /// Source/build revision.
    pub build_revision: String,
    /// Pinned semantic engine revision.
    pub ghostty_vt_revision: String,
    /// Target operating system.
    pub target_os: String,
    /// Target architecture.
    pub target_arch: String,
    /// Ordered capability probes.
    pub capabilities: Vec<Capability>,
}

impl CapabilityReport {
    /// Find one capability by its stable identifier.
    #[must_use]
    pub fn capability(&self, id: &str) -> Option<&Capability> {
        return self
            .capabilities
            .iter()
            .find(|capability| return capability.id == id);
    }

    /// Whether a named capability probe succeeded.
    #[must_use]
    pub fn is_ready(&self, id: &str) -> bool {
        return self
            .capability(id)
            .is_some_and(|capability| return capability.status == CapabilityStatus::Ready);
    }

    /// Require one capability from this exact bounded report.
    ///
    /// # Errors
    ///
    /// Returns an actionable typed error when the capability is blocked or the
    /// identifier is not part of the report.
    pub fn require(&self, id: &str) -> Result<()> {
        let capability = self.capability(id).ok_or_else(|| {
            return Error::InvalidReplay {
                reason: format!(
                    "unknown runtime capability `{id}`; expected one of {}",
                    CAPABILITY_IDS.join(", ")
                ),
            };
        })?;
        if capability.status == CapabilityStatus::Ready {
            return Ok(());
        }
        return Err(Error::CapabilityBlocked {
            id: capability.id.clone(),
            code: capability.code.clone(),
            detail: capability.detail.clone(),
            remediation: capability.remediation.clone().unwrap_or_else(|| {
                return "no automatic remediation is available".to_owned();
            }),
        });
    }
}

/// Probe and require one runtime capability.
///
/// # Errors
///
/// Returns an actionable typed error when the capability is blocked.
pub fn require_capability(id: &str) -> Result<()> {
    return probe_capabilities().require(id);
}

/// Probe once and require each runtime capability in caller-selected order.
///
/// # Errors
///
/// Returns the first actionable blocked-capability error.
pub fn require_capabilities(ids: &[&str]) -> Result<()> {
    let report = probe_capabilities();
    for id in ids {
        report.require(id)?;
    }
    return Ok(());
}

/// Probe built-in semantic support and optional local host tools.
///
#[must_use]
pub fn probe_capabilities() -> CapabilityReport {
    let semantic = GhosttyVt::new(2, 2, 0).and_then(|mut terminal| return terminal.snapshot());
    let semantic_capability = match semantic {
        Ok(_) => ready(
            SEMANTIC_CAPABILITY,
            "semantic_engine_ready",
            format!("pinned libghostty-vt {GHOSTTY_REVISION} created and snapshotted a terminal"),
        ),
        Err(error) => blocked(
            SEMANTIC_CAPABILITY,
            "semantic_engine_probe_failed",
            error.to_string(),
            "reinstall a build containing the pinned semantic engine",
        ),
    };
    let core = if cfg!(target_os = "linux") && executable("setsid").is_some() {
        ready(
            CORE_CAPABILITY,
            "core_linux_ready",
            "Linux PTY, owner-only evidence, Unix control, and detached-session tools are available",
        )
    } else {
        blocked(
            CORE_CAPABILITY,
            "core_host_dependency_missing",
            "the current core implementation requires Linux and util-linux setsid",
            "run on Linux and install util-linux",
        )
    };
    let missing_native_screenshot =
        missing(&["ghostty", "Xvfb", "dbus-run-session", "import", "ffprobe"]);
    let missing_native_video =
        missing(&["ghostty", "Xvfb", "dbus-run-session", "ffmpeg", "ffprobe"]);
    let missing_video = missing(&["ffmpeg", "ffprobe"]);
    let encoder = ffmpeg_has_encoder("libx264rgb");
    let (native_screenshot, native_video) = native_capabilities(
        cfg!(target_os = "linux"),
        &missing_native_screenshot,
        &missing_native_video,
        encoder,
    );
    let semantic_video = if semantic_capability.status == CapabilityStatus::Ready
        && missing_video.is_empty()
        && encoder
    {
        ready(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_tools_ready",
            "the semantic engine, FFmpeg/FFprobe, and libx264rgb are ready; native display tools are not required",
        )
    } else {
        blocked(
            SEMANTIC_VIDEO_CAPABILITY,
            "semantic_video_dependency_missing",
            format!(
                "semantic engine: {}; missing tools: {}; libx264rgb encoder: {}",
                if semantic_capability.status == CapabilityStatus::Ready {
                    "ready"
                } else {
                    "blocked"
                },
                missing_video.join(", "),
                if encoder { "available" } else { "unavailable" }
            ),
            "install FFmpeg/FFprobe with libx264rgb and use a build containing the pinned semantic engine",
        )
    };
    let presentation_encoder = ffmpeg_has_encoder("libx264");
    let presentation_ass = ffmpeg_has_filter("ass");
    let presentation = if missing_video.is_empty() && presentation_encoder && presentation_ass {
        ready(
            PRESENTATION_CAPABILITY,
            "presentation_tools_ready",
            "FFmpeg/FFprobe, H.264 delivery encoding, and the ASS/libass filter were found",
        )
    } else {
        blocked(
            PRESENTATION_CAPABILITY,
            "presentation_dependency_missing",
            format!(
                "missing tools: {}; libx264 encoder: {}; ASS/libass filter: {}",
                missing_video.join(", "),
                if presentation_encoder {
                    "available"
                } else {
                    "unavailable"
                },
                if presentation_ass {
                    "available"
                } else {
                    "unavailable"
                },
            ),
            "install FFmpeg/FFprobe with libx264 and libass support",
        )
    };
    let receipt = if executable("ssh-keygen").is_some() {
        ready(
            RECEIPT_SIGNING_CAPABILITY,
            "ssh_keygen_found",
            "OpenSSH ssh-keygen was found",
        )
    } else {
        blocked(
            RECEIPT_SIGNING_CAPABILITY,
            "ssh_keygen_missing",
            "OpenSSH ssh-keygen was not found",
            "install an OpenSSH client package",
        )
    };
    return CapabilityReport {
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_revision: env!("TERMTRACE_BUILD_REVISION").to_owned(),
        ghostty_vt_revision: GHOSTTY_REVISION.to_owned(),
        target_os: env::consts::OS.to_owned(),
        target_arch: env::consts::ARCH.to_owned(),
        capabilities: vec![
            core,
            semantic_capability,
            native_screenshot,
            native_video,
            semantic_video,
            presentation,
            receipt,
        ],
    };
}

fn native_capabilities(
    linux: bool,
    missing_screenshot: &[String],
    missing_video: &[String],
    video_encoder: bool,
) -> (Capability, Capability) {
    let screenshot = if linux && missing_screenshot.is_empty() {
        ready(
            NATIVE_SCREENSHOT_CAPABILITY,
            "native_host_tools_found",
            "Ghostty, Xvfb, D-Bus launcher, direct X11 PNG import, and FFprobe were found; capture performs the exact-grid runtime probe",
        )
    } else {
        blocked(
            NATIVE_SCREENSHOT_CAPABILITY,
            "native_host_tools_missing",
            format!("missing host tools: {}", missing_screenshot.join(", ")),
            "install Ghostty GTK, Xvfb, dbus-run-session, ImageMagick import, and FFprobe on Linux",
        )
    };
    let video = if linux && missing_video.is_empty() && video_encoder {
        ready(
            NATIVE_VIDEO_CAPABILITY,
            "native_video_tools_ready",
            "Ghostty, Xvfb, a D-Bus launcher, FFmpeg/FFprobe, and libx264rgb were found",
        )
    } else {
        blocked(
            NATIVE_VIDEO_CAPABILITY,
            "native_video_dependency_missing",
            format!(
                "missing tools: {}; libx264rgb encoder: {}",
                missing_video.join(", "),
                if video_encoder {
                    "available"
                } else {
                    "unavailable"
                }
            ),
            "install Ghostty GTK, Xvfb, dbus-run-session, and FFmpeg/FFprobe with libx264rgb on Linux",
        )
    };
    return (screenshot, video);
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
        .find(|candidate| return candidate.exists());
}

fn ffmpeg_has_encoder(encoder: &str) -> bool {
    let Some(ffmpeg) = executable("ffmpeg") else {
        return false;
    };
    return Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .is_some_and(|output| {
            return output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(encoder);
        });
}

fn ffmpeg_has_filter(filter: &str) -> bool {
    let Some(ffmpeg) = executable("ffmpeg") else {
        return false;
    };
    return Command::new(ffmpeg)
        .args(["-hide_banner", "-filters"])
        .output()
        .ok()
        .is_some_and(|output| {
            return output.status.success()
                && String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    return line.split_whitespace().any(|field| return field == filter);
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(capabilities: Vec<Capability>) -> CapabilityReport {
        return CapabilityReport {
            termtrace_version: "test".to_owned(),
            build_revision: "test".to_owned(),
            ghostty_vt_revision: "test".to_owned(),
            target_os: "linux".to_owned(),
            target_arch: "x86_64".to_owned(),
            capabilities,
        };
    }

    #[test]
    fn blocked_requirement_preserves_the_doctor_diagnostic() {
        let report = report(vec![blocked(
            NATIVE_VIDEO_CAPABILITY,
            "native_video_dependency_missing",
            "missing tools: ffmpeg",
            "install FFmpeg",
        )]);
        let error = report
            .require(NATIVE_VIDEO_CAPABILITY)
            .expect_err("blocked capability must fail");
        let Error::CapabilityBlocked {
            id,
            code,
            detail,
            remediation,
        } = error
        else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(id, NATIVE_VIDEO_CAPABILITY);
        assert_eq!(code, "native_video_dependency_missing");
        assert_eq!(detail, "missing tools: ffmpeg");
        assert_eq!(remediation, "install FFmpeg");
    }

    #[test]
    fn native_video_does_not_require_screenshot_only_import() {
        let (screenshot, video) = native_capabilities(true, &["import".to_owned()], &[], true);
        assert_eq!(screenshot.status, CapabilityStatus::Blocked);
        assert_eq!(video.status, CapabilityStatus::Ready);
    }

    #[test]
    fn stable_capability_ids_include_semantic_video() {
        assert!(CAPABILITY_IDS.contains(&SEMANTIC_VIDEO_CAPABILITY));
        assert_eq!(CAPABILITY_IDS.len(), 7);
    }
}
