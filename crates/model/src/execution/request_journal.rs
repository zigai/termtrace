//! Persistent mutation deduplication value contracts.

use crate::protocol::control::ControlResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Mutation journal record format identifier.
pub const MUTATION_JOURNAL_FORMAT: &str = "termtrace-mutation-request";
/// Mutation journal schema version.
pub const MUTATION_JOURNAL_VERSION: u32 = 3;

/// One durable mutation request outcome tied to a canonical event boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationJournalRecord {
    /// Stable record format identifier.
    pub format: String,
    /// Record schema version.
    pub version: u32,
    /// Contiguous zero-based journal sequence.
    pub seq: u64,
    /// Prior journal record hash, or 64 zeroes for the first record.
    pub prev_hash: String,
    /// Hash of this record body.
    pub hash: String,
    /// Caller-owned mutation request identifier.
    pub request_id: Uuid,
    /// BLAKE3 of the normalized serialized control command.
    pub command_hash: String,
    /// Canonical operation identifiers created by the request.
    pub operation_ids: Vec<Uuid>,
    /// Inclusive next-event boundary before command execution.
    pub event_start_seq: u64,
    /// Exclusive next-event boundary after command execution.
    pub event_end_seq: u64,
    /// Canonical event hash at `event_end_seq`.
    pub event_hash: String,
    /// Exact actor response returned for safe retry.
    pub response: ControlResponse,
}
