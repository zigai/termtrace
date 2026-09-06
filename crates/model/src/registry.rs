//! Owner-only named-session registry backed by canonical session identity.

use std::{ffi::OsString, os::unix::ffi::OsStringExt as _, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{execution::session::SessionGeometry, value::ByteString};

/// Registry-entry format identifier.
pub const REGISTRY_ENTRY_FORMAT: &str = "termtrace-session-registry-entry";
/// Registry-entry schema version.
pub const REGISTRY_ENTRY_VERSION: u32 = 1;

/// Exclusive, leased ownership of an alias while detached startup is pending.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AliasReservation {
    /// Stable reservation format identifier.
    pub format: String,
    /// Reservation schema version.
    pub version: u32,
    /// Validated friendly alias.
    pub alias: String,
    /// Unforgeable token required to promote or release this reservation.
    pub owner_token: Uuid,
    /// Reservation creation time from the local system clock.
    pub created_unix_ns: u128,
    /// Time after which another owner may reclaim the alias.
    pub lease_expires_unix_ns: u128,
    /// Exact path bytes allocated for the pending canonical session.
    pub pending_session: ByteString,
}

impl AliasReservation {
    /// Reconstruct the exact pending session path.
    pub fn session_path(&self) -> PathBuf {
        return PathBuf::from(OsString::from_vec(self.pending_session.as_bytes().to_vec()));
    }
}

/// One friendly name bound to an immutable canonical session identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    /// Stable entry format identifier.
    pub format: String,
    /// Entry schema version.
    pub version: u32,
    /// Validated friendly alias.
    pub alias: String,
    /// Exact canonical session-directory path bytes.
    pub canonical_session: ByteString,
    /// Immutable session ID read from `session.json` during registration.
    pub session_id: Uuid,
    /// Recorder PID when a supervisor supplied one.
    pub recorder_pid: Option<u32>,
    /// Linux process start-time ticks paired with `recorder_pid` against PID reuse.
    pub recorder_process_start_ticks: Option<u64>,
    /// Control protocol expected by the registering build.
    pub control_protocol_version: u32,
    /// Registry publication time from the local system clock.
    pub registered_unix_ns: u128,
}

impl RegistryEntry {
    /// Reconstruct the exact canonical session path.
    pub fn session_path(&self) -> PathBuf {
        return PathBuf::from(OsString::from_vec(
            self.canonical_session.as_bytes().to_vec(),
        ));
    }
}

/// Lifecycle classification derived without changing canonical evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryLifecycle {
    /// Recorder identity and control socket are present.
    Live,
    /// A completion manifest exists for the registered session ID.
    Completed,
    /// Evidence exists but has neither a live recorder identity nor completion.
    Incomplete,
    /// Socket or PID metadata remains but the recorder identity no longer matches.
    Stale,
    /// Canonical evidence is missing, insecure, or unreadable.
    MissingEvidence,
    /// The manifest session ID differs from the registered immutable identity.
    IdentityMismatch,
    /// Registry or control protocol format is unsupported.
    Incompatible,
}

impl RegistryLifecycle {
    /// Stable lowercase CLI and diagnostic spelling.
    pub fn as_str(self) -> &'static str {
        return match self {
            Self::Live => "live",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Stale => "stale",
            Self::MissingEvidence => "missing_evidence",
            Self::IdentityMismatch => "identity_mismatch",
            Self::Incompatible => "incompatible",
        };
    }
}

/// One registry listing row with evidence-derived session summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryListEntry {
    /// Friendly alias.
    pub alias: String,
    /// Exact canonical session path bytes.
    pub canonical_session: ByteString,
    /// Immutable registered session ID.
    pub session_id: Uuid,
    /// Effective command bytes when the manifest is readable.
    pub command: Option<Vec<ByteString>>,
    /// Effective working directory when the manifest is readable.
    pub working_directory: Option<ByteString>,
    /// Initial grid and pixel geometry when the manifest is readable.
    pub geometry: Option<SessionGeometry>,
    /// Session wall-clock origin when the manifest is readable.
    pub started_unix_ns: Option<u128>,
    /// Registry publication time.
    pub registered_unix_ns: u128,
    /// Current derived lifecycle classification.
    pub lifecycle: RegistryLifecycle,
    /// Focused diagnostic for unhealthy entries.
    pub diagnostic: Option<String>,
}

/// A named session path paired with the registry's evidence-derived lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrySessionResolution {
    /// Exact registered or pending session path.
    pub session: PathBuf,
    /// Current lifecycle classification.
    pub lifecycle: RegistryLifecycle,
    /// Focused explanation for incomplete or unhealthy states.
    pub diagnostic: Option<String>,
}

/// Collision-free paths allocated for one detached recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetachedSessionPaths {
    /// New canonical session path, not yet created.
    pub session: PathBuf,
    /// Owner-only mirrored PTY output log outside canonical evidence.
    pub stdout_log: PathBuf,
    /// Owner-only recorder diagnostic log outside canonical evidence.
    pub stderr_log: PathBuf,
}

/// One completed, unregistered state-root recording eligible for explicit deletion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GarbageCollectionCandidate {
    /// Canonical managed session directory.
    pub session: PathBuf,
    /// Immutable session identity.
    pub session_id: Uuid,
    /// Completion-file modification timestamp used for retention ordering.
    pub completed_unix_ns: u128,
    /// Complete bytes below the session directory.
    pub bytes: u64,
}

/// Safe state-root retention plan or applied deletion report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GarbageCollectionReport {
    /// Owner-only Termtrace state root considered.
    pub state_root: PathBuf,
    /// Whether eligible candidates were actually deleted.
    pub changes_applied: bool,
    /// Minimum completed-recording age in whole days.
    pub older_than_days: u64,
    /// Newest completed, unregistered recordings protected regardless of age.
    pub keep_latest: u64,
    /// Optional target for total completed managed-session bytes.
    #[serde(default)]
    pub max_completed_bytes: Option<u64>,
    /// Bytes in validated completed managed sessions at scan time, including named evidence.
    #[serde(default)]
    pub completed_bytes_before: u64,
    /// Completed bytes remaining if the reported candidates are deleted.
    #[serde(default)]
    pub completed_bytes_after_plan: u64,
    /// Bytes exceeding the target after planning because protected evidence is retained.
    #[serde(default)]
    pub quota_excess_bytes: u64,
    /// Completed managed recordings protected by a registry alias.
    pub protected_registered: u64,
    /// Live, incomplete, malformed, or non-directory entries never considered.
    pub skipped_non_completed: u64,
    /// Malformed or insecure registry records isolated from this scan.
    pub skipped_corrupt_registry_entries: u64,
    /// Exact eligible recordings, retained in the report even after deletion.
    pub candidates: Vec<GarbageCollectionCandidate>,
    /// Session directories removed when `changes_applied` is true.
    pub removed_sessions: u64,
    /// Detached supervisor log files removed with deleted sessions.
    pub removed_supervisor_files: u64,
    /// Stale alias registry bindings removed during collection.
    #[serde(default)]
    pub removed_stale_aliases: u64,
    /// Bytes reclaimed from canonical managed session directories.
    pub reclaimed_bytes: u64,
}
