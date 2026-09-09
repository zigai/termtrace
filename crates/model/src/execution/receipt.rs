//! Detached, externally signed evidence-root receipt contracts.

use crate::value::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Detached receipt relative path.
pub const EVIDENCE_RECEIPT_NAME: &str = "evidence-receipt.json";
/// Receipt envelope format.
pub const EVIDENCE_RECEIPT_FORMAT: &str = "termtrace-evidence-receipt";
/// Receipt schema version.
pub const EVIDENCE_RECEIPT_VERSION: u32 = 1;

/// Canonical unsigned roots covered by a detached receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceiptPayload {
    /// Domain identifier included in the signed object.
    pub domain: String,
    /// Payload schema version.
    pub version: u32,
    /// Immutable session identifier.
    pub session_id: Uuid,
    /// Exact immutable session-manifest BLAKE3.
    pub session_manifest_hash: String,
    /// Final canonical event BLAKE3.
    pub final_event_hash: String,
    /// Number of canonical events.
    pub event_count: u64,
    /// Exact completion-document BLAKE3.
    pub completion_hash: String,
    /// Number of artifact-root records covered by this checkpoint.
    pub artifact_root_count: u64,
    /// Artifact-root record hash at the checkpoint, when non-empty.
    pub artifact_root_hash: Option<String>,
    /// Signer identity interpreted through the caller's trust policy.
    pub signer_key_id: String,
    /// Unix wall-clock time supplied by the signing host.
    pub created_unix_ns: u128,
    /// Explicit source of the creation time.
    pub creation_time_source: String,
    /// Pinned canonical payload encoding.
    pub canonicalization: String,
}

/// Detached signature metadata and bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSignature {
    /// External signature scheme.
    pub scheme: String,
    /// Namespace supplied to the external signature implementation.
    pub namespace: String,
    /// Signer identity interpreted through an allowed-signers policy.
    pub key_id: String,
    /// Exact detached signature bytes.
    pub value: ByteString,
}

/// Detached evidence receipt envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceipt {
    /// Stable envelope format identifier.
    pub format: String,
    /// Envelope schema version.
    pub version: u32,
    /// Canonical unsigned payload.
    pub payload: EvidenceReceiptPayload,
    /// Detached signature excluded from the signed payload.
    pub signature: EvidenceSignature,
}

/// Cryptographic and trust-policy results kept deliberately separate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptVerification {
    /// Signer identity asserted by the receipt.
    pub signer_key_id: String,
    /// Whether the detached signature is valid for the exact payload bytes.
    pub cryptographic_valid: bool,
    /// Whether a configured allowed-signers policy accepts that identity/key.
    pub signer_trusted: bool,
    /// Artifact-root prefix covered by the signed checkpoint.
    pub artifact_root_count: u64,
}
