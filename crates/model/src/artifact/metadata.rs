//! Shared metadata contracts for derived artifact members and source ranges.

use crate::execution::event_log::CanonicalBoundary;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Sensitivity classification for one derived artifact member.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSensitivity {
    /// Member contains or may reveal terminal content.
    SensitiveTerminalData,
    /// Member contains only non-content operational metadata.
    OperationalMetadata,
}

/// Whole-file integrity metadata embedded in a derived manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMember {
    /// Normalized relative path below the artifact directory.
    pub path: String,
    /// Exact byte length.
    pub byte_length: u64,
    /// BLAKE3 of the complete file.
    pub blake3: String,
    /// Stable media type.
    pub media_type: String,
    /// Public schema identifier, when the file is structured data.
    pub schema_type: Option<String>,
    /// Sensitivity classification.
    pub sensitivity: ArtifactSensitivity,
    /// Relationship to the derivation.
    pub relationship: String,
}

/// Half-open canonical source range bound into an artifact catalog root.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSourceRange {
    /// Inclusive canonical monotonic start in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive canonical monotonic end in nanoseconds.
    pub end_monotonic_ns: u64,
}

/// One hash-chained artifact-root entry bound to completion or an explicit prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRootRecord {
    /// Stable record format identifier.
    pub format: String,
    /// Record schema version.
    pub version: u32,
    /// Contiguous zero-based catalog sequence.
    pub seq: u64,
    /// Prior catalog record hash, or 64 zeroes for the first entry.
    pub prev_hash: String,
    /// Hash of this record body and prior hash.
    pub hash: String,
    /// Unique derivation identifier.
    pub derivation_id: Uuid,
    /// Stable derivation family.
    pub derivation_type: String,
    /// Artifact directory relative to the canonical session.
    pub artifact_directory: String,
    /// BLAKE3 of the exact artifact manifest bytes.
    pub manifest_hash: String,
    /// Session-manifest root committed by the event header.
    pub session_manifest_hash: String,
    /// Source event boundary used by the derivation.
    pub source_event_hash: Option<String>,
    /// Selected half-open canonical source interval, if any.
    pub source_range: Option<ArtifactSourceRange>,
    /// Explicit canonical prefix for a derivation made before source completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_prefix: Option<CanonicalBoundary>,
}
