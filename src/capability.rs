//! Runtime capability inventory and bounded host probes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{env, ffi::OsStr, path::PathBuf, process::Command};

use crate::{ghostty_vt::GhosttyVt, replay::GHOSTTY_REVISION};

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
    /// Whether a named capability probe succeeded.
    #[must_use]
    pub fn is_ready(&self, id: &str) -> bool {
        return self.capabilities.iter().any(|capability| {
            return capability.id == id && capability.status == CapabilityStatus::Ready;
        });
    }
}

/// Probe built-in semantic support and optional local host tools.
///
#[must_use]
pub fn probe_capabilities() -> CapabilityReport {
    let semantic = GhosttyVt::new(2, 2, 0).and_then(|mut terminal| return terminal.snapshot());
    let semantic_capability = match semantic {
        Ok(_) => ready(
            "semantic",
            "semantic_engine_ready",
            format!("pinned libghostty-vt {GHOSTTY_REVISION} created and snapshotted a terminal"),
        ),
        Err(error) => blocked(
            "semantic",
            "semantic_engine_probe_failed",
            error.to_string(),
            "reinstall a build containing the pinned semantic engine",
        ),
    };
    let core = if cfg!(target_os = "linux") && executable("setsid").is_some() {
        ready(
            "core",
            "core_linux_ready",
            "Linux PTY, owner-only evidence, Unix control, and detached-session tools are available",
        )
    } else {
        blocked(
            "core",
            "core_host_dependency_missing",
            "the current core implementation requires Linux and util-linux setsid",
            "run on Linux and install util-linux",
        )
    };
    let missing_native = missing(&["ghostty", "Xvfb", "dbus-run-session"]);
    let native_screenshot = if cfg!(target_os = "linux") && missing_native.is_empty() {
        ready(
            "native-screenshot",
            "native_host_tools_found",
            "Ghostty, Xvfb, and D-Bus launcher were found; capture performs the exact-grid runtime probe",
        )
    } else {
        blocked(
            "native-screenshot",
            "native_host_tools_missing",
            format!("missing host tools: {}", missing_native.join(", ")),
            "install Ghostty GTK, Xvfb, and dbus-run-session on Linux",
        )
    };
    let missing_video = missing(&["ffmpeg", "ffprobe"]);
    let encoder = ffmpeg_has_encoder("libx264rgb");
    let native_video = if native_screenshot.status == CapabilityStatus::Ready
        && missing_video.is_empty()
        && encoder
    {
        ready(
            "native-video",
            "native_video_tools_ready",
            "native screenshot tools, FFmpeg/FFprobe, and libx264rgb were found",
        )
    } else {
        blocked(
            "native-video",
            "native_video_dependency_missing",
            format!(
                "missing tools: {}; libx264rgb encoder: {}",
                missing_video.join(", "),
                if encoder { "available" } else { "unavailable" }
            ),
            "install FFmpeg/FFprobe with libx264rgb plus the native screenshot dependencies",
        )
    };
    let receipt = if executable("ssh-keygen").is_some() {
        ready(
            "receipt-signing",
            "ssh_keygen_found",
            "OpenSSH ssh-keygen was found",
        )
    } else {
        blocked(
            "receipt-signing",
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
            receipt,
        ],
    };
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
