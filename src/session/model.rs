use super::*;

/// Session manifest format identifier.
pub const SESSION_MANIFEST_FORMAT: &str = "termtrace-session";
/// Session manifest schema version.
pub const SESSION_MANIFEST_VERSION: u32 = 12;

/// Current tmux-owned-pane provenance contract.
pub const TMUX_PANE_PROVENANCE_VERSION: u32 = 1;
/// Canonical root-level Ghostty replay-profile artifact.
pub const GHOSTTY_REPLAY_PROFILE_NAME: &str = "ghostty-profile.ghostty";
/// Completion manifest format identifier.
pub const SESSION_COMPLETION_FORMAT: &str = "termtrace-completion";
/// Completion manifest schema version.
pub const SESSION_COMPLETION_VERSION: u32 = 5;
/// Validation contract used when the completion manifest was created.
pub const COMPLETION_VALIDATION_VERSION: u32 = 1;
/// Verification report format identifier.
pub const VERIFICATION_REPORT_FORMAT: &str = "termtrace-verification";
/// Verification report schema version.
pub const VERIFICATION_REPORT_VERSION: u32 = 4;

/// Canonical runtime resource limits for one recording.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// Soft output budget that triggers termination after all observed bytes are retained.
    pub output_bytes: Option<u64>,
}

/// Canonical resource-limit kinds with implemented completion semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimitKind {
    /// Observed PTY output byte count.
    OutputBytes,
}

/// Resource-limit trigger retained in immutable completion evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimitOutcome {
    /// Stable limit kind.
    pub kind: ResourceLimitKind,
    /// Configured trigger threshold.
    pub limit: u64,
    /// Exact observed count when the host triggered termination.
    pub observed: u64,
}

/// Requested depth for an offline evidence verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLevel {
    /// Parse all canonical files and enforce their versioned structure.
    Structure,
    /// Verify the manifest binding, event chain, completion, and terminal state.
    Content,
    /// Verify content plus the chain-bound durability policy and recorded outcomes.
    DurabilityRecord,
    /// Verify content plus a detached signature and configured signer trust.
    Signature,
}

/// One stable verification diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Human-readable context that is not intended for programmatic matching.
    pub message: String,
}

/// Machine-readable result from offline session verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationReport {
    /// Stable report format identifier.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session directory checked by the verifier.
    pub session: PathBuf,
    /// Requested verification depth.
    pub level: VerificationLevel,
    /// Whether every invariant at the requested level passed.
    pub valid: bool,
    /// Immutable session identifier, when readable.
    pub session_id: Option<Uuid>,
    /// Exact session-manifest hash, when readable.
    pub session_manifest_hash: Option<String>,
    /// Final canonical event hash, when readable.
    pub final_event_hash: Option<String>,
    /// Number of locally cataloged artifact roots verified at content level.
    pub artifact_roots_verified: u64,
    /// Whether artifact authenticity was established from a trusted checkpoint.
    pub artifact_authenticity_established: bool,
    /// Whether canonical session authenticity was established from a trusted receipt.
    pub session_authenticity_established: bool,
    /// State of the disposable mutation deduplication acceleration journal.
    pub mutation_journal_status: MutationJournalStatus,
    /// Number of mutation outcomes verified against canonical ranges.
    pub mutation_journal_records_verified: u64,
    /// Journal-only diagnostic when acceleration is invalid.
    pub mutation_journal_error: Option<String>,
    /// Whether a detached evidence receipt was present.
    pub signature_receipt_present: bool,
    /// Signer identity asserted by the detached receipt.
    pub signature_signer_key_id: Option<String>,
    /// Whether the detached signature matches the exact receipt payload.
    pub signature_cryptographic_valid: bool,
    /// Whether the configured trust policy accepts the signer identity/key.
    pub signature_signer_trusted: bool,
    /// Stable findings; empty when verification succeeds.
    pub findings: Vec<VerificationFinding>,
    /// Page coordinates when CLI output intentionally contains a bounded subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_page: Option<OutputPage>,
}

/// Offline status of the disposable mutation request journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MutationJournalStatus {
    /// The acceleration journal is absent; canonical evidence remains valid.
    Missing,
    /// Every journal record agrees with canonical event evidence.
    Verified,
    /// Journal acceleration is corrupt or stale and must not be reused.
    Invalid,
}

/// Immutable provenance written before the child is spawned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionManifest {
    /// Stable format identifier.
    pub format: String,
    /// Schema version.
    pub version: u32,
    /// Session identifier shared with the event log.
    pub session_id: Uuid,
    /// Friendly alias supplied at creation, never used as canonical identity.
    pub creation_alias: Option<String>,
    /// Evidence-bound predecessor when this session is an immutable successor.
    pub lineage: Option<SessionLineage>,
    /// Optional provenance when this session canonically imports a byte stream.
    #[serde(default)]
    pub import_provenance: Option<ImportProvenance>,
    /// tmux pane identity when this command was launched through native tmux recording.
    #[serde(default)]
    pub tmux_pane: Option<TmuxPaneProvenance>,
    /// Unix wall-clock origin in nanoseconds.
    pub started_unix_ns: u128,
    /// Exact executable and argument bytes.
    pub command: Vec<ByteString>,
    /// Exact effective working-directory bytes, when the launch context can provide them.
    pub working_directory: Option<ByteString>,
    /// Initial PTY geometry.
    pub geometry: SessionGeometry,
    /// Selection source and optional capture target for the initial geometry.
    #[serde(default)]
    pub geometry_provenance: GeometryProvenance,
    /// Canonical per-recording Ghostty profile, when explicitly resolved.
    #[serde(default)]
    pub ghostty_visual: Option<GhosttyVisualProvenance>,
    /// Value-free child-environment policy; selected values live in execution provenance.
    pub child_environment: ChildEnvironmentPolicy,
    /// Sensitivity-aware execution provenance and its explicit collection policy.
    pub execution_provenance: ExecutionProvenance,
    /// Termtrace package version.
    pub termtrace_version: String,
    /// Build target operating system.
    pub operating_system: String,
    /// Build target architecture.
    pub architecture: String,
    /// Canonical event-log relative path.
    pub event_log: String,
    /// Linux abstract control-socket name for the live recorder.
    pub control_socket: String,
    /// Persistent mutation deduplication journal relative path.
    pub request_journal: String,
    /// Explicit sensitivity warning.
    pub contains_sensitive_terminal_data: bool,
    /// Chain-bound output persistence policy.
    pub durability_policy: DurabilityPolicy,
    /// Chain-bound runtime resource limits.
    pub resource_limits: ResourceLimits,
}

/// Recorder failure retained when the child could not complete normally.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionFailure {
    /// Stable failure category.
    pub code: String,
    /// Human-readable failure context.
    pub message: String,
}

/// Immutable completion facts written after the final event is durable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCompletion {
    /// Stable format identifier.
    pub format: String,
    /// Completion schema version.
    pub version: u32,
    /// Validation contract applied before publication.
    pub validation_version: u32,
    /// Session identifier.
    pub session_id: Uuid,
    /// BLAKE3 of the exact immutable session manifest bytes.
    pub session_manifest_hash: String,
    /// BLAKE3 of the exact event-log header bytes.
    pub event_header_hash: String,
    /// Normal exit code when available.
    pub exit_code: Option<i32>,
    /// Signal name when available.
    pub signal_name: Option<String>,
    /// Total raw output bytes.
    pub output_bytes: u64,
    /// Hash of the final canonical event.
    pub final_event_hash: String,
    /// Number of canonical events.
    pub event_count: u64,
    /// Sequence number of the required terminal `Exit` event.
    pub terminal_event_seq: u64,
    /// Recorder-observed persistence behavior and final sync result.
    pub durability: DurabilityRecord,
    /// Resource limit that caused host termination, when one was reached.
    pub resource_limit: Option<ResourceLimitOutcome>,
    /// Fatal recorder failure, absent for normal child completion.
    pub failure: Option<SessionFailure>,
}
