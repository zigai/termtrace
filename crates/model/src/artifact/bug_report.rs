//! Bounded, redacted bug-report evidence contracts.

use crate::{
    artifact::metadata::ArtifactMember, execution::session::SessionGeometry, value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Redacted session metadata included in a bug report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportMetadata {
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Friendly creation alias, when supplied.
    pub creation_alias: Option<String>,
    /// Unix wall-clock recording origin in nanoseconds.
    pub started_unix_ns: u128,
    /// Initial PTY geometry.
    pub geometry: SessionGeometry,
    /// Termtrace package version used to record the session.
    pub termtrace_version: String,
    /// Recording host operating system.
    pub operating_system: String,
    /// Recording host architecture.
    pub architecture: String,
    /// Child exit code, when available.
    pub exit_code: Option<i32>,
    /// Child signal name, when available.
    pub signal_name: Option<String>,
    /// Total canonical output bytes without their contents.
    pub output_bytes: u64,
    /// Canonical event count.
    pub event_count: u64,
    /// Stable recorder failure code, when present.
    pub failure_code: Option<String>,
}

/// Exact canonical source reference without copied canonical byte streams.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportSessionReference {
    /// Canonical source path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub canonical_session: ByteString,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical event hash.
    pub final_event_hash: String,
    /// Canonical event count.
    pub event_count: u64,
}

/// Whole-file manifest for a bounded bug-report directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BugReportManifest {
    /// Stable manifest format.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Canonical source identity.
    pub session_id: Uuid,
    /// Exact canonical source path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub canonical_session: ByteString,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Raw input/output event byte streams are intentionally excluded.
    pub includes_raw_streams: bool,
    /// Whether a semantic screenshot was explicitly requested.
    pub includes_screenshot: bool,
    /// Whether the export includes last-action analysis and diagnostics.
    pub includes_last_action: bool,
    /// Whole-file member inventory.
    pub members: Vec<ArtifactMember>,
}
