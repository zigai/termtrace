//! Stable metadata for bounded agent-facing collections.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Deterministic page coordinates for one ordered collection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputPage {
    /// Effective zero-based offset, clamped to the collection length.
    pub offset: usize,
    /// Requested maximum, or none when every remaining item was requested.
    pub limit: Option<usize>,
    /// Complete matching collection length before pagination.
    pub total: usize,
    /// Number of items included in this response.
    pub returned: usize,
    /// Whether more matching items remain after this page.
    pub truncated: bool,
    /// Offset for the next page when more items remain.
    pub next_offset: Option<usize>,
}
