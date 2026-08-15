//! Sensitivity-aware execution provenance value contracts.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::value::ByteString;

/// Built-in execution-provenance collection policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenancePolicyKind {
    /// Retain only essential terminal values and inventory common locale context.
    #[default]
    Minimal,
    /// Retain values needed to recreate common locale and timezone behavior.
    Reproducible,
    /// Apply only caller-supplied environment capture rules.
    Custom,
}

/// How one selected provenance value is represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceCapture {
    /// Retain the exact value and its hash.
    Value,
    /// Retain only a value hash for later equality checks.
    Hash,
    /// Retain only the variable name and whether it was set.
    Name,
    /// Explicitly document that the selected value was not captured.
    NotCaptured,
}

/// Sensitivity classification for one provenance item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSensitivity {
    /// Conventional terminal, locale, or timezone configuration.
    PublicConfiguration,
    /// Caller-selected data that may contain private or proprietary content.
    PotentiallySensitive,
}

/// Explicit immutable provenance policy.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvenancePolicy {
    /// Built-in policy behavior.
    pub kind: ProvenancePolicyKind,
    /// Custom environment rules; required to be empty for built-in policies.
    pub environment: BTreeMap<String, ProvenanceCapture>,
}

/// Captured or inventoried environment provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentProvenance {
    /// Representation selected by the policy.
    pub capture: ProvenanceCapture,
    /// Whether the variable existed in the effective child environment.
    pub present: bool,
    /// Exact bytes when capture is `value`.
    pub value: Option<ByteString>,
    /// BLAKE3 of the exact value bytes for `value` or `hash` capture.
    pub value_hash: Option<String>,
    /// Sensitivity classification used by exporters and reviewers.
    pub sensitivity: ProvenanceSensitivity,
}

/// Policy plus its immutable environment inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProvenance {
    /// Policy that selected the inventory.
    pub policy: ProvenancePolicy,
    /// Selected effective child-environment facts, ordered by name.
    pub environment: BTreeMap<String, EnvironmentProvenance>,
    /// Resolved executable identity.
    pub executable: FileProvenance,
    /// Interpreter-invoked script when it can be identified without guessing.
    pub invoked_file: Option<FileProvenance>,
    /// Recorder, kernel, and local identity facts.
    pub runtime: RuntimeProvenance,
}

/// Whether content identity was retained for a resolved file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentIdentityStatus {
    /// The policy did not request file content hashing.
    NotRequested,
    /// BLAKE3 was computed from the complete file.
    Captured,
    /// The path could not be resolved to an executable or regular file.
    Unresolved,
    /// A resolved file could not be read completely.
    Unavailable,
}

/// Exact requested path plus resolved content identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileProvenance {
    /// Bytes supplied by the caller or discovered as an interpreter argument.
    pub requested: ByteString,
    /// Canonical resolved path bytes when resolution succeeded.
    pub resolved: Option<ByteString>,
    /// Complete-file BLAKE3 under a policy that requests content identity.
    pub content_hash: Option<String>,
    /// Explicit content identity outcome.
    pub content_identity: ContentIdentityStatus,
}

/// Runtime kernel identity returned by the host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KernelProvenance {
    /// Operating-system name.
    pub system: String,
    /// Kernel release.
    pub release: String,
    /// Kernel build/version string.
    pub version: String,
    /// Runtime machine architecture.
    pub machine: String,
}

/// Recorder build and binary identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecorderProvenance {
    /// Package version.
    pub version: String,
    /// Source revision embedded at build time.
    pub source_revision: String,
    /// Pinned semantic-engine revision.
    pub semantic_engine_revision: String,
    /// Running recorder binary identity.
    pub binary: FileProvenance,
}

/// Recorder process identity and host context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProvenance {
    /// Real user ID.
    pub uid: u32,
    /// Effective user ID.
    pub effective_uid: u32,
    /// Real group ID.
    pub gid: u32,
    /// Effective group ID.
    pub effective_gid: u32,
    /// Whether the effective identity is privileged root.
    pub privileged: bool,
    /// Process umask read without modifying it, when the host exposes it.
    pub umask: Option<u32>,
    /// Runtime kernel identity.
    pub kernel: KernelProvenance,
    /// Recorder build/binary identity.
    pub recorder: RecorderProvenance,
}
