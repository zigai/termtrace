//! Capability reports, system metadata, and schema generation services.

use crate::Result;
use termtrace_engine::{
    analysis::capability::{probe_capabilities as probe_engine_capabilities, require},
    Error as EngineError,
};
use termtrace_evidence::receipt::receipt_signing_capability;
use termtrace_media::{
    capability::{derived_media_capabilities, presentation_capability, semantic_video_capability},
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
    #[must_use]
    pub fn probe_capabilities() -> CapabilityReport {
        let mut external = derived_media_capabilities().to_vec();
        external.extend(native_capabilities(true, true));
        external.push(receipt_signing_capability());

        return probe_engine_capabilities(external);
    }

    /// Probe only capability `id` and fail unless it is ready.
    ///
    /// # Errors
    /// Returns a wrapped [`EngineError::CapabilityBlocked`] when the capability is unavailable.
    pub fn require(id: &str) -> Result<()> {
        let capability = match id {
            SEMANTIC_VIDEO_CAPABILITY => Some(semantic_video_capability()),
            PRESENTATION_CAPABILITY => Some(presentation_capability()),
            RECEIPT_SIGNING_CAPABILITY => Some(receipt_signing_capability()),
            NATIVE_SCREENSHOT_CAPABILITY | NATIVE_VIDEO_CAPABILITY => native_capabilities(
                id == NATIVE_SCREENSHOT_CAPABILITY,
                id == NATIVE_VIDEO_CAPABILITY,
            )
            .next(),
            _ => None,
        };
        let Some(capability) = capability else {
            return Ok(require(&probe_engine_capabilities([]), id)?);
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
