//! Runtime capability composition and semantic-engine probing.

use crate::{Error, Result};
use std::{collections::BTreeMap, env};
use termtrace_model::{
    protocol::capability::{
        Capability, CapabilityReport, CapabilityStatus, CAPABILITY_IDS, CORE_CAPABILITY,
        SEMANTIC_CAPABILITY,
    },
    terminal::replay::GHOSTTY_REVISION,
};
use termtrace_terminal::GhosttyVt;

/// Require one capability from an exact bounded report.
pub fn require(report: &CapabilityReport, id: &str) -> Result<()> {
    let capability = report.capability(id).ok_or_else(|| {
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
        remediation: capability
            .remediation
            .clone()
            .unwrap_or_else(|| return "no automatic remediation is available".to_owned()),
    });
}

/// Probe engine-owned capabilities and merge bounded probes supplied by other owners.
///
/// The caller supplies media, rendering, export, and receipt-signing probes. Duplicate,
/// unknown, or omitted external identifiers are reported as blocked rather than trusted.
#[must_use]
pub fn probe_capabilities(external: impl IntoIterator<Item = Capability>) -> CapabilityReport {
    let mut supplied: BTreeMap<String, Capability> = BTreeMap::new();
    for capability in external {
        supplied.entry(capability.id.clone()).or_insert(capability);
    }

    let semantic = match GhosttyVt::new(2, 2, 0).and_then(|mut terminal| return terminal.snapshot())
    {
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

    let core = if cfg!(target_os = "linux") {
        ready(
            CORE_CAPABILITY,
            "core_linux_ready",
            "Linux PTY and owner-only evidence primitives are supported",
        )
    } else {
        blocked(
            CORE_CAPABILITY,
            "core_host_unsupported",
            "the current core implementation requires Linux",
            "run on Linux",
        )
    };

    let mut capabilities: Vec<Capability> = Vec::with_capacity(CAPABILITY_IDS.len());
    for id in CAPABILITY_IDS {
        let capability = match *id {
            CORE_CAPABILITY => core.clone(),
            SEMANTIC_CAPABILITY => semantic.clone(),
            external_id => supplied.remove(external_id).unwrap_or_else(|| {
                return blocked(
                    external_id,
                    "capability_probe_not_supplied",
                    format!("the `{external_id}` owner did not supply a bounded probe result"),
                    "run capability probing through the composed Termtrace facade",
                );
            }),
        };
        capabilities.push(capability);
    }

    return CapabilityReport {
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_revision: option_env!("TERMTRACE_BUILD_REVISION")
            .unwrap_or("unknown")
            .to_owned(),
        ghostty_vt_revision: GHOSTTY_REVISION.to_owned(),
        target_os: env::consts::OS.to_owned(),
        target_arch: env::consts::ARCH.to_owned(),
        capabilities,
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

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_model::protocol::capability::NATIVE_VIDEO_CAPABILITY;

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

        let error =
            require(&report, NATIVE_VIDEO_CAPABILITY).expect_err("blocked capability must fail");
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
}
