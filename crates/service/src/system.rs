//! Capability reports, system metadata, and schema generation services.

use termtrace_engine::analysis::capability::probe_capabilities as probe_engine_capabilities;
use termtrace_evidence::receipt::receipt_signing_capability;
use termtrace_media::{
    capability::derived_media_capabilities, native_capture::native_capabilities,
};
use termtrace_model::protocol::capability::CapabilityReport;

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
}
