//! Canonical session manifest, completion, and verification contracts.

use crate::{
    execution::environment::ChildEnvironmentPolicy,
    execution::event_log::{DurabilityPolicy, DurabilityRecord},
    execution::provenance::ExecutionProvenance,
    value::pagination::OutputPage,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Session manifest format identifier.
pub const SESSION_MANIFEST_FORMAT: &str = "termtrace-session";
/// Session manifest schema version.
pub const SESSION_MANIFEST_VERSION: u32 = 12;
/// Current tmux-owned-pane provenance contract.
pub const TMUX_PANE_PROVENANCE_VERSION: u32 = 1;
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

/// Behavior when recording fails while an owned application is still running.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFailurePolicy {
    /// Stop and reap an owned test or explicitly strict recording.
    #[default]
    Stop,
    /// Preserve the PTY connection and application while reporting degraded evidence.
    Preserve,
}

/// Canonical runtime resource limits for one recording.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// Soft output budget that triggers termination after all observed bytes are retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    /// Maximum session duration in milliseconds before termination is triggered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    /// Maximum idle duration in milliseconds without input, output, or control activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_ms: Option<u64>,
}

/// Canonical resource-limit kinds with implemented completion semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimitKind {
    /// Observed PTY output byte count.
    OutputBytes,
    /// Wall-clock session duration limit.
    MaxDuration,
    /// Inactivity idle timeout.
    IdleTimeout,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "The versioned verification report exposes separate validity, authenticity, receipt, and trust results"
)]
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
    /// Explicit application-lifecycle policy for recorder failures.
    #[serde(default)]
    pub failure_policy: RecordingFailurePolicy,
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

/// Where a session is in its life, observed without changing canonical evidence.
///
/// This is the one session-phase vocabulary. Registry listings, CLI output,
/// and live-control failures all report it rather than inventing their own.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
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

impl SessionPhase {
    /// Whether the recorder has published its final state.
    pub const fn is_completed(self) -> bool {
        return matches!(self, Self::Completed);
    }

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

/// Initial PTY geometry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionGeometry {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Total pixel width.
    pub pixel_width: u16,
    /// Total pixel height.
    pub pixel_height: u16,
}

/// Origin of the initial terminal geometry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeometrySource {
    /// Explicit columns, rows, and cell dimensions.
    #[default]
    Manual,
    /// Dimensions inherited from the controlling terminal.
    Inherited,
    /// Dimensions measured from Ghostty at a target viewport size.
    GhosttyFit,
}

/// Pixel viewport intended for native visual capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaptureTarget {
    /// Viewport width in pixels.
    pub width: u16,
    /// Viewport height in pixels.
    pub height: u16,
}

/// Provenance for automatic terminal geometry selection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeometryProvenance {
    /// How the initial geometry was selected.
    pub source: GeometrySource,
    /// Intended native-capture viewport, when selected during recording.
    pub capture_target: Option<CaptureTarget>,
}

/// Focused Termtrace overrides applied to a Ghostty visual profile.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GhosttyVisualOverrides {
    /// Regular terminal font family.
    pub font_family: Option<String>,
    /// Font size in points, represented exactly as a Ghostty config value.
    pub font_size: Option<String>,
    /// Symmetric horizontal window padding in pixels.
    pub padding_x: Option<u16>,
    /// Symmetric vertical window padding in pixels.
    pub padding_y: Option<u16>,
    /// Default foreground color in Ghostty syntax.
    pub foreground: Option<String>,
    /// Default background color in Ghostty syntax.
    pub background: Option<String>,
}

impl GhosttyVisualOverrides {
    /// Whether no convenience override was supplied.
    pub fn is_empty(&self) -> bool {
        return self.font_family.is_none()
            && self.font_size.is_none()
            && self.padding_x.is_none()
            && self.padding_y.is_none()
            && self.foreground.is_none()
            && self.background.is_none();
    }
}

/// Persisted provenance for a canonical per-recording Ghostty profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GhosttyVisualProvenance {
    /// Original custom configuration path bytes, when supplied.
    pub source_config: Option<ByteString>,
    /// Focused overrides applied after resolving the source configuration.
    pub overrides: GhosttyVisualOverrides,
    /// Relative canonical replay-profile artifact.
    pub replay_profile: String,
    /// BLAKE3 of the exact replay-profile input bytes.
    pub replay_profile_hash: String,
    /// BLAKE3 of the exact effective configuration bytes freshly reported by Ghostty.
    pub effective_config_hash: String,
}

/// Immutable evidence binding from a successor session to its predecessor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionLineage {
    /// Canonical predecessor session identifier.
    pub predecessor_session_id: Uuid,
    /// BLAKE3 of the exact predecessor `session.json` bytes.
    pub predecessor_manifest_hash: String,
    /// Chain root of the predecessor's final canonical event.
    pub predecessor_final_event_hash: String,
    /// Stable reason for creating the successor.
    pub reason: String,
}

/// Chain-bound provenance for bytes imported from a noncanonical ANSI stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportProvenance {
    /// Stable source class, currently `ansi_stream`.
    pub source_kind: String,
    /// Exact imported byte count.
    pub byte_count: u64,
    /// BLAKE3 of the exact imported bytes.
    pub blake3: String,
    /// Caller-facing path bytes or `-` for standard input.
    pub source: ByteString,
}

/// How the native tmux wrapper selected the command running in its child PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TmuxPaneLaunchKind {
    /// tmux created an ordinary pane and Termtrace restored its configured login shell.
    DefaultShell,
    /// tmux had a nonempty default command which Termtrace preserved through its configured shell.
    DefaultCommand,
    /// The caller supplied an exact executable and argument vector.
    ExplicitArgv,
    /// The caller supplied one command interpreted by `/bin/sh -c`.
    ShellCommand,
}

/// Immutable tmux identity captured before a Termtrace-owned pane child is spawned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxPaneProvenance {
    /// Version of this tmux pane provenance contract.
    pub version: u32,
    /// tmux version reported by the server.
    pub tmux_version: String,
    /// Exact tmux server socket path bytes.
    pub socket_path: ByteString,
    /// tmux server process identifier.
    pub server_pid: u32,
    /// Stable tmux session identifier at pane launch.
    pub session_id: String,
    /// Session label at pane launch; labels may change later.
    pub session_name: ByteString,
    /// Unix timestamp in seconds at which tmux created the session.
    pub session_created_unix_seconds: u64,
    /// Stable tmux window identifier at pane launch.
    pub window_id: String,
    /// Window index at pane launch.
    pub window_index: u32,
    /// Window label at pane launch; labels may change later.
    pub window_name: ByteString,
    /// Stable tmux pane identifier.
    pub pane_id: String,
    /// Pane index within the window at launch.
    pub pane_index: u32,
    /// Outer pane process identifier assigned to the Termtrace wrapper.
    pub pane_pid: u32,
    /// tmux pane width inherited by the child PTY.
    pub pane_width: u16,
    /// tmux pane height inherited by the child PTY.
    pub pane_height: u16,
    /// Exact tmux start-command display captured for diagnostics.
    pub pane_start_command: ByteString,
    /// How the wrapper selected the recorded child command.
    pub launch_kind: TmuxPaneLaunchKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::layout;

    #[test]
    fn ghostty_profile_provenance_retains_the_split_hash_contract() {
        let provenance = GhosttyVisualProvenance {
            source_config: None,
            overrides: GhosttyVisualOverrides::default(),
            replay_profile: layout::GHOSTTY_REPLAY_PROFILE.to_owned(),
            replay_profile_hash: "replay-hash".to_owned(),
            effective_config_hash: "effective-hash".to_owned(),
        };
        let encoded = serde_json::to_value(&provenance).expect("profile provenance JSON");
        assert_eq!(
            encoded.get("replay_profile"),
            Some(&serde_json::json!(layout::GHOSTTY_REPLAY_PROFILE))
        );
        assert_eq!(
            encoded.get("replay_profile_hash"),
            Some(&serde_json::json!("replay-hash"))
        );
        assert_eq!(
            encoded.get("effective_config_hash"),
            Some(&serde_json::json!("effective-hash"))
        );
        assert!(encoded.get("effective_config").is_none());

        let obsolete = serde_json::json!({
            "source_config": null,
            "overrides": GhosttyVisualOverrides::default(),
            "effective_config": "ghostty-config.txt",
            "effective_config_hash": "obsolete-overloaded-hash"
        });

        assert!(serde_json::from_value::<GhosttyVisualProvenance>(obsolete).is_err());

        let misspelled_override = serde_json::json!({
            "source_config": null,
            "overrides": {
                "font_family": null,
                "font_size": null,
                "padding_x": null,
                "padding_y": null,
                "foreground": null,
                "backround": "#000000"
            },
            "replay_profile": layout::GHOSTTY_REPLAY_PROFILE,
            "replay_profile_hash": "a".repeat(64),
            "effective_config_hash": "b".repeat(64)
        });

        assert!(serde_json::from_value::<GhosttyVisualProvenance>(misspelled_override).is_err());
    }

    #[test]
    fn geometry_provenance_has_no_visual_hash_authority() {
        let provenance = GeometryProvenance {
            source: GeometrySource::GhosttyFit,
            capture_target: Some(CaptureTarget {
                width: 1280,
                height: 720,
            }),
        };
        let encoded = serde_json::to_value(&provenance).expect("geometry provenance JSON");
        assert!(encoded.get("effective_config_hash").is_none());
        assert!(encoded.get("ghostty_config_hash").is_none());

        for obsolete_field in ["ghostty_config_hash", "effective_config_hash"] {
            let mut obsolete = serde_json::json!({
                "source": "ghostty_fit",
                "capture_target": null
            });
            obsolete.as_object_mut().expect("geometry object").insert(
                obsolete_field.to_owned(),
                serde_json::json!("obsolete-hash"),
            );

            assert!(serde_json::from_value::<GeometryProvenance>(obsolete).is_err());
        }
    }
}
