//! Capability reports, system metadata, and schema generation services.

use crate::{Error, Result};
use std::env::consts::{ARCH, OS};
use std::sync::atomic::AtomicBool;
use termtrace_engine::{
    analysis::capability::{
        probe_capabilities as probe_engine_capabilities, require, semantic_capability,
    },
    Error as EngineError,
};
use termtrace_evidence::receipt::receipt_signing_capability;
use termtrace_media::{
    capability::{derivative_capability, presentation_capability, semantic_video_capability},
    native_capture::native_capabilities,
};
use termtrace_model::artifact::derivative::{DerivativeFormat, GifLoop};
use termtrace_model::protocol::capability::{
    Capability, CapabilityReport, CapabilityStatus, GIF_DELIVERY_CAPABILITY,
    NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY, PRESENTATION_CAPABILITY,
    RECEIPT_SIGNING_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY, WEBM_DELIVERY_CAPABILITY,
};
use termtrace_model::{
    protocol::control::CONTROL_PROTOCOL_VERSION, terminal::replay::GHOSTTY_REVISION,
};
use termtrace_schema::catalog;

const GIF: DerivativeFormat = DerivativeFormat::Gif {
    loop_policy: GifLoop::Infinite,
};

const WEBM: DerivativeFormat = DerivativeFormat::Webm {};

fn delivery_capability(format: DerivativeFormat) -> Capability {
    return derivative_capability(format, false, false);
}

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
            delivery_capability(GIF),
            delivery_capability(WEBM),
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
            GIF_DELIVERY_CAPABILITY => Some(delivery_capability(GIF)),
            WEBM_DELIVERY_CAPABILITY => Some(delivery_capability(WEBM)),
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

/// Build and component versions of this Termtrace library.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct Versions {
    /// Termtrace package version.
    pub termtrace: String,
    /// Control protocol version.
    pub control_protocol: String,
    /// Pinned Ghostty VT revision.
    pub ghostty_revision: String,
    /// Target operating system.
    pub target_os: String,
    /// Target CPU architecture.
    pub target_arch: String,
}

impl SystemService {
    /// Typed component/build versions without starting probes.
    #[must_use]
    pub fn versions() -> Versions {
        return Versions {
            termtrace: env!("CARGO_PKG_VERSION").into(),
            control_protocol: CONTROL_PROTOCOL_VERSION.to_string(),
            ghostty_revision: GHOSTTY_REVISION.into(),
            target_os: OS.into(),
            target_arch: ARCH.into(),
        };
    }

    /// Names from the existing canonical schema catalog.
    /// # Errors
    /// Propagates catalog registration failures.
    pub fn schemas() -> Result<Vec<String>> {
        return catalog()
            .map(|entries| {
                return entries
                    .iter()
                    .map(|entry| return entry.filename.to_owned())
                    .collect();
            })
            .map_err(|error| {
                return Error::InvalidOperation {
                    reason: error.to_string(),
                    source: Some(Box::new(error)),
                };
            });
    }

    /// One canonical schema as explicit JSON-domain data.
    /// # Errors
    /// Unknown names and catalog/generation failures propagate.
    pub fn schema(name: &str) -> Result<serde_json::Value> {
        let entries = catalog().map_err(|error| {
            return Error::InvalidOperation {
                reason: error.to_string(),
                source: Some(Box::new(error)),
            };
        })?;
        let entry = entries
            .iter()
            .find(|entry| return entry.filename == name)
            .ok_or_else(|| {
                return Error::InvalidOperation {
                    reason: format!("unknown schema {name}"),
                    source: None,
                };
            })?;
        let schema = entry.generate().map_err(|error| {
            return Error::InvalidOperation {
                reason: error.to_string(),
                source: Some(Box::new(error)),
            };
        })?;

        return serde_json::from_slice(&schema.json).map_err(|source| {
            return Error::InvalidJson {
                path: name.into(),
                json_path: "$".into(),
                source,
            };
        });
    }
}
