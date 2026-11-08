//! Owner-only named-session registry backed by canonical session identity.

use crate::execution::{
    event_log::DurabilityPolicy,
    provenance::{ProvenanceCapture, ProvenancePolicyKind},
};
use crate::{
    execution::session::{SessionGeometry, SessionPhase},
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, os::unix::ffi::OsStringExt as _, path::PathBuf};
use uuid::Uuid;

/// Registry-entry format identifier.
pub const REGISTRY_ENTRY_FORMAT: &str = "termtrace-session-registry-entry";
/// Registry-entry schema version.
pub const REGISTRY_ENTRY_VERSION: u32 = 2;

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
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
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
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
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

/// One registry listing row with evidence-derived session summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryListEntry {
    /// Friendly alias.
    pub alias: String,
    /// Exact canonical session path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub canonical_session: ByteString,
    /// Immutable registered session ID.
    pub session_id: Uuid,
    /// Effective command bytes when the manifest is readable.
    pub command: Option<Vec<ByteString>>,
    /// Effective working directory when the manifest is readable.
    #[serde(with = "crate::value::path_bytes::option")]
    #[schemars(with = "Option<crate::value::path_bytes::PathValue>")]
    pub working_directory: Option<ByteString>,
    /// Initial grid and pixel geometry when the manifest is readable.
    pub geometry: Option<SessionGeometry>,
    /// Session wall-clock origin when the manifest is readable.
    pub started_unix_ns: Option<u128>,
    /// Registry publication time.
    pub registered_unix_ns: u128,
    /// Current derived lifecycle classification.
    pub lifecycle: SessionPhase,
    /// Focused diagnostic for unhealthy entries.
    pub diagnostic: Option<String>,
}

/// A named session path paired with the registry's evidence-derived lifecycle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegistrySessionResolution {
    /// Exact registered or pending session path.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub session: PathBuf,
    /// Current lifecycle classification.
    pub lifecycle: SessionPhase,
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
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
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
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
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

/// Recorder restart durability override. Pascal-case wire names preserve the existing CLI handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum RestartDurability {
    /// Synchronize each event.
    #[serde(alias = "strict")]
    Strict,
    /// Bound pending time and bytes.
    #[serde(alias = "bounded")]
    Bounded,
    /// Synchronize mutation/lifecycle boundaries.
    #[serde(alias = "throughput")]
    Throughput,
}

/// Recorder restart provenance override, preserving existing internal wire names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum RestartProvenance {
    /// Minimal provenance.
    #[serde(alias = "minimal")]
    Minimal,
    /// Reproducible provenance.
    #[serde(alias = "reproducible")]
    Reproducible,
    /// Explicit environment rules.
    #[serde(alias = "custom")]
    Custom,
}

/// Typed successor configuration. Absent fields keep predecessor values; clear flags explicitly
/// remove optional limits. This is a recorder-launch DTO, not a persisted evidence format.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent explicit clearing and environment-change permissions preserve the existing restart wire contract"
)]
pub struct RestartConfiguration {
    /// New exact working directory, absent to keep.
    #[serde(with = "crate::value::path_bytes::option")]
    #[schemars(with = "Option<crate::value::path_bytes::PathValue>")]
    pub cwd: Option<ByteString>,
    /// New columns, absent to keep.
    pub cols: Option<u16>,
    /// New rows, absent to keep.
    pub rows: Option<u16>,
    /// New cell width, absent to keep.
    pub cell_width: Option<u16>,
    /// New cell height, absent to keep.
    pub cell_height: Option<u16>,
    /// New TERM, absent to keep.
    pub term: Option<String>,
    /// New durability mode, absent to keep.
    pub durability: Option<RestartDurability>,
    /// Bounded durability interval override.
    pub durability_max_interval_ms: Option<u64>,
    /// Bounded durability byte override.
    pub durability_max_uncommitted_bytes: Option<u64>,
    /// New provenance collection mode.
    pub provenance: Option<RestartProvenance>,
    /// Explicit provenance environment rules.
    pub provenance_environment: Vec<(String, ProvenanceCapture)>,
    /// New output budget.
    pub output_byte_budget: Option<u64>,
    /// Clear the output budget rather than keeping it.
    pub clear_output_byte_budget: bool,
    /// New wall-clock duration limit.
    pub max_duration_ms: Option<u64>,
    /// Clear the wall-clock limit.
    pub clear_max_duration: bool,
    /// New inactivity limit.
    pub idle_timeout_ms: Option<u64>,
    /// Clear the inactivity limit.
    pub clear_idle_timeout: bool,
    /// Explicitly permit an intentionally changed environment.
    pub allow_environment_change: bool,
    /// New exact executable/arguments; absent to keep.
    pub command: Option<Vec<ByteString>>,
}

impl RestartDurability {
    /// Build the existing canonical durability policy.
    #[must_use]
    pub fn policy(
        self,
        maximum_interval_ms: u64,
        maximum_uncommitted_bytes: u64,
    ) -> DurabilityPolicy {
        use crate::execution::event_log::{DurabilityMode, DurabilityPolicy};
        return match self {
            Self::Strict => DurabilityPolicy {
                mode: DurabilityMode::Strict,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
            Self::Bounded => DurabilityPolicy {
                mode: DurabilityMode::Bounded,
                maximum_interval_ms: Some(maximum_interval_ms),
                maximum_uncommitted_bytes: Some(maximum_uncommitted_bytes),
            },
            Self::Throughput => DurabilityPolicy {
                mode: DurabilityMode::Throughput,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
        };
    }
}

impl From<RestartProvenance> for ProvenancePolicyKind {
    fn from(value: RestartProvenance) -> Self {
        return match value {
            RestartProvenance::Minimal => Self::Minimal,
            RestartProvenance::Reproducible => Self::Reproducible,
            RestartProvenance::Custom => Self::Custom,
        };
    }
}

/// Shared restart result, retaining the predecessor and compare-and-retargeted successor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestartReport {
    /// Alias compare-and-retargeted after validated readiness and lineage.
    pub alias: String,
    /// Preserved predecessor binding.
    pub predecessor: RegistryEntry,
    /// New binding.
    pub successor: RegistryEntry,
    /// Exact owner-only stdout log path.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub stdout_log: ByteString,
    /// Exact owner-only stderr log path.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub stderr_log: ByteString,
    /// True only after successful retargeting.
    pub ready: bool,
}
