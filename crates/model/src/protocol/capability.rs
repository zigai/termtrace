//! Runtime capability inventory and bounded host probes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_capability_ids_include_semantic_video() {
        assert!(CAPABILITY_IDS.contains(&SEMANTIC_VIDEO_CAPABILITY));
        assert_eq!(CAPABILITY_IDS.len(), 7);
    }
}
