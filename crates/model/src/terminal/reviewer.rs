//! Read-only semantic forensic reviewer with safe terminal rendering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable scriptable reviewer command inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewerCapabilities {
    /// Reviewer contract version.
    pub version: u32,
    /// Interactive command identifiers.
    pub commands: Vec<String>,
    /// Safety boundary for recorded terminal content.
    pub rendering_policy: String,
}

impl Default for ReviewerCapabilities {
    fn default() -> Self {
        return Self {
            version: 1,
            commands: [
                "next_state",
                "previous_state",
                "toggle_presented_only",
                "next_marker",
                "previous_marker",
                "next_input",
                "previous_input",
                "next_search_match",
                "previous_search_match",
                "toggle_help_palette",
                "inspect_native_correlation",
                "mouse_scroll_navigation",
                "export_selected_state",
                "quit",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rendering_policy: "semantic_cells_only_no_raw_replay".to_owned(),
        };
    }
}

/// Noninteractive reviewer source summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSummary {
    /// Immutable completed session directory.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub canonical_session: PathBuf,
    /// Number of semantic boundary states.
    pub state_count: u64,
    /// Number of presentation-eligible states.
    pub presented_state_count: u64,
    /// Number of canonical markers.
    pub marker_count: u64,
    /// Number of honest direct or scheduled native correlations.
    pub native_correlation_count: u64,
    /// Final semantic state hash.
    pub final_state_hash: String,
    /// Safety boundary used for viewport rendering.
    pub rendering_policy: String,
}

/// One semantic viewport text match with a copyable canonical selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSearchMatch {
    /// Exact semantic state identifier.
    pub state_id: u64,
    /// Canonical event sequence associated with the state.
    pub event_seq: Option<u64>,
    /// Whether the state was presentation-eligible.
    pub presented: bool,
    /// Exact semantic state hash.
    pub state_hash: String,
    /// Ready-to-run selector arguments.
    pub selector: String,
}

/// Stable noninteractive reviewer text-search result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSearchResult {
    /// Immutable completed session directory.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub canonical_session: PathBuf,
    /// Exact viewport substring query.
    pub query: String,
    /// Ordered semantic state matches.
    pub matches: Vec<ReviewSearchMatch>,
}

/// Manifest for one explicit reviewer state export.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExportManifest {
    /// Reviewer export contract version.
    pub version: u32,
    /// Immutable source session.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub canonical_session: PathBuf,
    /// Exact exported semantic state identifier.
    pub state_id: u64,
    /// Exact semantic state hash.
    pub state_hash: String,
    /// Canonical source head anchoring the export.
    pub source_final_event_hash: String,
    /// BLAKE3 of `state.json`.
    pub state_json_blake3: String,
    /// BLAKE3 of `screen.txt`.
    pub screen_text_blake3: String,
}
