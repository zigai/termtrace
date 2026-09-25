//! Versioned newline-framed process driver contracts.

use crate::{
    artifact::bundle::{BundleManifest, BundleProfile},
    execution::environment::EnvironmentSpec,
    execution::event::EventProjection,
    execution::session::{
        CaptureTarget, ResourceLimits, SessionCompletion, VerificationLevel, VerificationReport,
    },
    protocol::control::{ControlCommand, ControlData, ControlErrorDetails},
    protocol::mutation::MutationEffect,
    terminal::analysis::{DiagnoseReport, DiagnosticCheck},
    terminal::diff::SemanticDiff,
    terminal::inspection::WaitOutcome,
    terminal::observation::{Observation, StatePoint},
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::timeline::TimelineEntry,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Current JSONL driver protocol version.
pub const DRIVER_PROTOCOL_VERSION: u32 = 6;
/// Maximum JSON bytes accepted in one newline-delimited request frame.
pub const DRIVER_MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum JSON bytes emitted in one newline-delimited response frame.
pub const DRIVER_MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Explicit screenshot truth path; no fallback occurs between variants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriverScreenshotRenderer {
    /// Deterministic semantic PNG.
    Semantic,
    /// Direct native frame after exact endpoint replay.
    Native,
}

/// One correlated driver request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriverRequest {
    /// Driver protocol version.
    pub version: u32,
    /// Caller-owned retry/correlation identifier.
    pub request_id: Uuid,
    /// Typed driver operation.
    pub method: DriverMethod,
}

/// Supported initial high-level driver methods.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverMethod {
    /// Return protocol and evidence capabilities.
    Hello,
    /// Launch a fresh canonical session through the high-level Rust facade.
    Launch {
        /// Owner-only root receiving collision-free evidence.
        evidence_root: PathBuf,
        /// Exact executable and argument bytes.
        command: Vec<ByteString>,
        /// Initial terminal columns.
        cols: u16,
        /// Initial terminal rows.
        rows: u16,
        /// Optional child working directory.
        working_directory: Option<PathBuf>,
        /// Deterministic child-environment policy; values travel only in driver stdin.
        #[serde(default)]
        environment: EnvironmentSpec,
        /// Non-truncating runtime resource limits.
        #[serde(default)]
        resource_limits: ResourceLimits,
    },
    /// Attach a process-local handle to a canonical session or registry name.
    Attach {
        /// Name, `name:` reference, `path:` reference, or canonical path.
        session: PathBuf,
    },
    /// Forward one typed live control command using the driver request ID.
    Control {
        /// Process-local session handle.
        handle: Uuid,
        /// Typed actor command.
        command: ControlCommand,
        /// Optional optimistic presented-state guard.
        expected_presented_state_hash: Option<String>,
    },
    /// Wait for exact bytes in canonical child output history.
    WaitOutput {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact byte sequence to find.
        data: ByteString,
        /// Bounded wait duration.
        timeout_ms: u64,
    },
    /// Read immutable completion evidence for an attached session.
    Completion {
        /// Process-local session handle.
        handle: Uuid,
    },
    /// Wait for and validate immutable completion evidence.
    Complete {
        /// Process-local session handle.
        handle: Uuid,
        /// Bounded wait duration.
        timeout_ms: u64,
    },
    /// Delete completed evidence launched by this driver under explicit policy.
    DiscardCompleted {
        /// Process-local owned session handle.
        handle: Uuid,
    },
    /// Observe one live or completed semantic state point.
    Observe {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact shared state selector.
        point: StatePoint,
        /// Permit explicitly selected hidden synchronized-output state.
        include_hidden: bool,
    },
    /// Project one exact bounded rich frame from a live or completed state.
    ScreenFrame {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact shared state selector.
        point: StatePoint,
        /// Permit explicitly selected hidden synchronized-output state.
        include_hidden: bool,
        /// Requested rich detail and exact optional viewport region.
        projection: ScreenProjectionRequest,
    },
    /// Build one bounded page of the completed canonical/semantic timeline.
    Timeline {
        /// Process-local session handle.
        handle: Uuid,
        /// Include non-presented semantic states.
        all_states: bool,
        /// Include exact lossless VT frames.
        controls: bool,
        /// Zero-based entry offset.
        offset: u64,
        /// Maximum returned entries, from 1 through 4096.
        limit: u16,
    },
    /// Compare two completed semantic endpoints.
    Diff {
        /// Process-local session handle.
        handle: Uuid,
        /// Earlier state point.
        from: StatePoint,
        /// Later state point.
        to: StatePoint,
    },
    /// Run conservative completed-session detectors.
    Diagnose {
        /// Process-local session handle.
        handle: Uuid,
        /// Empty means every detector.
        checks: Vec<DiagnosticCheck>,
        /// Broad repaint threshold from 1 through 100.
        broad_change_percent: u8,
    },
    /// Read bounded canonical event projections.
    Events {
        /// Process-local session handle.
        handle: Uuid,
        /// Inclusive first event sequence.
        from_event_seq: u64,
        /// Maximum returned events, bounded by the driver.
        limit: u16,
        /// Elide exact byte payloads.
        metadata_only: bool,
    },
    /// Publish one semantic or native screenshot artifact.
    Screenshot {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact selected state point.
        point: StatePoint,
        /// No-replace artifact directory.
        destination: PathBuf,
        /// Explicit truth renderer.
        renderer: DriverScreenshotRenderer,
        /// Optional strict native viewport; absent runs the solver.
        viewport: Option<CaptureTarget>,
        /// Maximum semantic screenshot artifact bytes, including its manifest; native requires absent.
        max_bytes: Option<u64>,
    },
    /// Verify attached completed evidence without mutation.
    Verify {
        /// Process-local session handle.
        handle: Uuid,
        /// Requested validation depth.
        level: VerificationLevel,
    },
    /// Publish one sensitivity-profiled evidence handoff.
    Bundle {
        /// Process-local session handle.
        handle: Uuid,
        /// New no-replace bundle directory.
        destination: PathBuf,
        /// Explicit sensitivity profile.
        profile: BundleProfile,
        /// Maximum generated bundle bytes, including its manifest.
        max_bytes: Option<u64>,
    },
    /// Release only the process-local handle.
    Detach {
        /// Process-local session handle.
        handle: Uuid,
    },
}

/// Exactly one correlated driver response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriverResponse {
    /// Driver protocol version.
    pub version: u32,
    /// Identifier copied from the request.
    pub request_id: Uuid,
    /// Typed success or stable error.
    pub result: DriverResult,
}

/// Driver success or failure envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverResult {
    /// Successful method result.
    Ok {
        /// Method-specific data.
        data: DriverData,
    },
    /// Stable structured failure.
    Error {
        /// Machine-readable code.
        code: String,
        /// Human-readable diagnostic.
        message: String,
        /// What the method did before it failed; callers derive retry from it.
        effect: MutationEffect,
        /// Canonical evidence path when known.
        evidence: Option<PathBuf>,
        /// Actor-provided ordering or attempted-prefix details when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<ControlErrorDetails>,
    },
}

/// Typed driver result payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverData {
    /// Protocol/version handshake.
    Hello {
        /// Driver protocol version.
        driver_protocol_version: u32,
        /// Live actor protocol version.
        control_protocol_version: u32,
        /// Canonical event format version.
        event_log_version: u32,
        /// Semantic state format version.
        state_format_version: u32,
        /// Stable supported methods.
        methods: Vec<String>,
        /// Maximum newline-framed request bytes.
        max_request_bytes: u64,
        /// Maximum newline-framed response bytes.
        max_response_bytes: u64,
    },
    /// Attached process-local handle and immutable identity.
    Attached {
        /// Process-local handle.
        handle: Uuid,
        /// Immutable canonical session identity.
        session_id: Uuid,
        /// Canonical path.
        canonical_session: PathBuf,
        /// Whether completion evidence already exists.
        completed: bool,
    },
    /// Result forwarded from the live actor.
    Control {
        /// Exact actor result data.
        data: ControlData,
    },
    /// Exact canonical output wait result.
    OutputWait {
        /// First match and its canonical position.
        outcome: WaitOutcome,
    },
    /// Immutable completion evidence.
    Completion {
        /// Validated completion envelope.
        completion: SessionCompletion,
    },
    /// Truth-labeled semantic observation.
    Observation {
        /// Shared observation envelope.
        observation: Box<Observation>,
    },
    /// Bounded rich frame projected at one exact source point.
    ScreenFrame {
        /// Exact owned rich projection.
        frame: ScreenFrame,
    },
    /// Ordered timeline projection page.
    Timeline {
        /// Requested entries in canonical order.
        entries: Vec<TimelineEntry>,
        /// Next zero-based entry offset, absent at the end.
        next_offset: Option<u64>,
    },
    /// Complete semantic endpoint diff.
    Diff {
        /// Source-bound semantic difference.
        difference: Box<SemanticDiff>,
    },
    /// Conservative diagnostic report.
    Diagnose {
        /// Source-bound findings.
        report: DiagnoseReport,
    },
    /// Bounded canonical event projection page.
    Events {
        /// Ordered event projections.
        events: Vec<EventProjection>,
        /// Next event sequence when more source may exist.
        next_event_seq: u64,
    },
    /// Published screenshot artifact handle.
    Artifact {
        /// `semantic_screenshot` or `native_screenshot`.
        artifact_type: String,
        /// Atomically committed artifact directory.
        directory: PathBuf,
        /// Primary PNG member.
        primary: PathBuf,
        /// Manifest path.
        manifest: PathBuf,
        /// Exact truth-layer label.
        truth_layer: String,
    },
    /// Offline canonical verification report.
    Verification {
        /// Stable complete report, including invalid findings.
        report: Box<VerificationReport>,
    },
    /// Published sensitivity-profiled bundle.
    Bundle {
        /// Atomically committed directory.
        directory: PathBuf,
        /// Complete bundle manifest.
        manifest: BundleManifest,
    },
    /// Process-local handle release acknowledgement.
    Detached,
    /// Explicit completed-evidence deletion acknowledgement.
    Discarded {
        /// Confirms that the canonical directory was removed.
        evidence_removed: bool,
    },
}
