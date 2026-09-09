//! Bundle manifest, profile, and verification policy contracts.

use crate::{
    artifact::metadata::{ArtifactMember, ArtifactSensitivity},
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Reserved manifest filename at a bundle root.
pub const BUNDLE_MANIFEST_NAME: &str = "manifest.json";
/// Stable bundle manifest format identifier.
pub const BUNDLE_FORMAT: &str = "termtrace-bundle";
/// Current bundle manifest format version.
pub const BUNDLE_VERSION: u32 = 1;

/// Fixed bundle content policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundleProfile {
    /// Latest screen observation without canonical raw events.
    Screen,
    /// Screen plus metadata-only canonical event projections and verification.
    Debug,
    /// Complete top-level canonical session payload plus verification.
    Forensic,
}

/// Caller-owned source and policy expectations for untrusted bundle verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleVerificationPolicy {
    /// Trusted canonical source session selected by the verifier.
    pub source_session: PathBuf,
    /// Exact bundle profile the verifier expects.
    pub profile: BundleProfile,
}

impl BundleVerificationPolicy {
    /// Construct explicit verification expectations.
    #[must_use]
    pub fn new(source_session: impl Into<PathBuf>, profile: BundleProfile) -> Self {
        return Self {
            source_session: source_session.into(),
            profile,
        };
    }
}

/// One planned member and its sensitivity before publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundlePlanMember {
    /// Relative output path.
    pub path: String,
    /// Explicit sensitivity class.
    pub sensitivity: ArtifactSensitivity,
    /// Why the member is selected.
    pub relationship: String,
}

/// Dry-run selection inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundlePlan {
    /// Stable plan format.
    pub format: String,
    /// Plan schema version.
    pub version: u32,
    /// Selected policy.
    pub profile: BundleProfile,
    /// Whether canonical raw evidence is included.
    pub includes_canonical_evidence: bool,
    /// Planned files.
    pub members: Vec<BundlePlanMember>,
}

/// Top-level whole-file bundle manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    /// Stable manifest format.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Selected policy.
    pub profile: BundleProfile,
    /// Immutable canonical session identity.
    pub session_id: Uuid,
    /// Exact source session path bytes.
    pub canonical_session: ByteString,
    /// Final canonical event hash.
    pub source_final_event_hash: String,
    /// Whether canonical raw evidence is included.
    pub includes_canonical_evidence: bool,
    /// Whole-file inventory; unmanifested payloads are invalid.
    pub members: Vec<ArtifactMember>,
}
