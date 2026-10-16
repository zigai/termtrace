//! Capability reports, system metadata, and schema generation services.

use crate::Result;
use std::sync::atomic::AtomicBool;
use termtrace_engine::{
    analysis::capability::{
        probe_capabilities as probe_engine_capabilities, require, semantic_capability,
    },
    Error as EngineError,
};
use termtrace_evidence::receipt::receipt_signing_capability;
use termtrace_media::{
    capability::{presentation_capability, semantic_video_capability},
    native_capture::native_capabilities,
};
use termtrace_model::protocol::capability::{
    CapabilityReport, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY,
    PRESENTATION_CAPABILITY, RECEIPT_SIGNING_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
};

/// System capability and environment introspection service.
pub struct SystemService;

impl SystemService {
    /// Probe all runtime capabilities across engine, media, and evidence signers.
    ///
    /// Setting `cancelled` stops the native capture probe early; it then reports blocked.
    #[must_use]
    pub fn probe_capabilities(cancelled: &AtomicBool) -> CapabilityReport {
        let semantic = semantic_capability();
        let mut external = vec![
            semantic_video_capability(semantic.status == CapabilityStatus::Ready),
            presentation_capability(),
        ];
        external.extend(native_capabilities(true, true, cancelled));
        external.push(receipt_signing_capability());

        return probe_engine_capabilities(semantic, external);
    }

    /// Probe only capability `id` and fail unless it is ready.
    ///
    /// Setting `cancelled` stops a native capture probe early; it then reports blocked.
    ///
    /// # Errors
    /// Returns a wrapped [`EngineError::CapabilityBlocked`] when the capability is unavailable.
    pub fn require(id: &str, cancelled: &AtomicBool) -> Result<()> {
        let capability = match id {
            SEMANTIC_VIDEO_CAPABILITY => Some(semantic_video_capability(
                semantic_capability().status == CapabilityStatus::Ready,
            )),
            PRESENTATION_CAPABILITY => Some(presentation_capability()),
            RECEIPT_SIGNING_CAPABILITY => Some(receipt_signing_capability()),
            NATIVE_SCREENSHOT_CAPABILITY | NATIVE_VIDEO_CAPABILITY => native_capabilities(
                id == NATIVE_SCREENSHOT_CAPABILITY,
                id == NATIVE_VIDEO_CAPABILITY,
                cancelled,
            )
            .next(),
            _ => None,
        };
        let Some(capability) = capability else {
            return Ok(require(
                &probe_engine_capabilities(semantic_capability(), []),
                id,
            )?);
        };

        if capability.status == CapabilityStatus::Ready {
            return Ok(());
        }
        return Err(EngineError::CapabilityBlocked {
            id: capability.id,
            code: capability.code,
            detail: capability.detail,
            remediation: capability
                .remediation
                .unwrap_or_else(|| return "no automatic remediation is available".to_owned()),
        }
        .into());
    }
}
