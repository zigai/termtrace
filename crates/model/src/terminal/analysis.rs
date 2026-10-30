//! Stable evidence-backed analysis report contracts.

use crate::{
    execution::event::OperationStatus, terminal::diff::ValueChange,
    terminal::observation::StatePoint, terminal::replay::StateRecord, terminal::state::CursorState,
    terminal::vt::VtToken, value::pagination::OutputPage, value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Canonical request and intent/result boundaries defining one input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionBoundary {
    /// Caller-owned request identifier when the action was durably journaled.
    pub request_id: Option<Uuid>,
    /// Ordered canonical operation identifiers for every input atom.
    pub operation_ids: Vec<Uuid>,
    /// Number of input atoms in the request.
    pub atom_count: u32,
    /// Inclusive canonical event boundary before request execution.
    pub request_event_start_seq: u64,
    /// Exclusive canonical event boundary after request execution and postcondition evaluation.
    pub request_event_end_seq: u64,
    /// First canonical input-intent event sequence.
    pub intent_event_seq: u64,
    /// Last canonical input-result event sequence.
    pub result_event_seq: u64,
    /// Intended input byte count without the input bytes themselves.
    pub intended_bytes: u64,
    /// Aggregate applied or failed result.
    pub status: OperationStatus,
    /// Bytes confirmed written across every atom.
    pub bytes_written: u64,
    /// First stable result error code, when any atom failed.
    pub error_code: Option<String>,
}

/// Aggregate child output attributable to the last action boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionOutputSummary {
    /// Canonical output-event count after the input intent.
    pub event_count: u64,
    /// Exact output byte count without the output bytes themselves.
    pub byte_count: u64,
}

/// Canonical output byte that carried an OSC 133 mark.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandMark {
    /// Canonical output event holding the mark's completing byte.
    pub event_seq: u64,
    /// Offset of that byte within the event.
    pub event_byte_offset: u64,
}

/// First shell command that OSC 133 marks report after the action's input intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionShellCommand {
    /// Decoded command line when the shell sent one.
    pub command: Option<ByteString>,
    /// Mark where the command started running, absent when the shell skipped it.
    pub started: Option<CommandMark>,
    /// Mark where the command finished, absent while it still runs.
    pub ended: Option<CommandMark>,
    /// Exit code the shell reported with the end mark.
    pub exit_code: Option<i32>,
}

/// One changed ANSI or DEC terminal mode.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionModeChange {
    /// Numeric terminal mode identifier.
    pub number: u16,
    /// `true` for ANSI and `false` for DEC-private mode.
    pub ansi: bool,
    /// Earlier set/reset value.
    pub before: bool,
    /// Later set/reset value.
    pub after: bool,
}

/// One bounded warning derived from canonical action evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionWarning {
    /// Stable warning code.
    pub code: String,
    /// Conservative confidence label.
    pub confidence: FindingConfidence,
    /// Bounded neutral evidence facts.
    pub facts: Vec<String>,
}

/// Compact semantic result of the most recent canonical input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Canonical input action boundary.
    pub action: LastActionBoundary,
    /// Earlier semantic state identifier.
    pub from_state_id: u64,
    /// Later semantic state identifier.
    pub to_state_id: u64,
    /// Sorted zero-based viewport rows with semantic cell changes.
    pub rows_changed: Vec<u16>,
    /// Total changed semantic cells.
    pub cells_changed: u64,
    /// Complete cursor movement when the cursor changed.
    pub cursor_movement: Option<ValueChange<CursorState>>,
    /// Sorted terminal mode changes.
    pub mode_changes: Vec<ActionModeChange>,
    /// Child output generated after the canonical input intent.
    pub output_generated: ActionOutputSummary,
    /// Whether the resulting semantic screen was presentation eligible.
    pub presentation_eligible: bool,
    /// Number of presentation-eligible semantic boundaries after the action.
    pub presentation_eligible_states: u64,
    /// Shell command boundaries after the action, when the shell sent OSC 133 marks.
    pub shell_command: Option<ActionShellCommand>,
    /// Bounded conservative warnings.
    pub warnings: Vec<ActionWarning>,
    /// Whether additional warnings were omitted.
    pub warnings_truncated: bool,
}

/// Diagnostics restricted to the semantic and token range after the last input action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionDiagnosticsReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical source hash.
    pub source_final_event_hash: String,
    /// Canonical input action boundary.
    pub action: LastActionBoundary,
    /// Earlier semantic state identifier.
    pub from_state_id: u64,
    /// Later semantic state identifier.
    pub to_state_id: u64,
    /// Bounded findings supported inside the selected action range.
    pub findings: Vec<DiagnosticFinding>,
    /// Whether additional findings were omitted.
    pub findings_truncated: bool,
}

/// Both task-level reports derived in one canonical replay/diagnostic pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LastActionAnalysis {
    /// Compact action diff and warnings.
    pub inspection: LastActionReport,
    /// Range-restricted diagnostics.
    pub diagnostics: LastActionDiagnosticsReport,
}

/// Conservative confidence label for a derived finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingConfidence {
    /// Directly encoded control and semantic outcome agree.
    High,
    /// Ordered facts support a candidate but do not prove user perception.
    Medium,
}

/// One deterministic evidence-backed diagnostic finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Conservative confidence label.
    pub confidence: FindingConfidence,
    /// VT token IDs supporting the finding.
    pub token_ids: Vec<u64>,
    /// Exact neutral facts, without pixel-level inference.
    pub facts: Vec<String>,
}

/// Causal evidence around one canonical state point.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainReport {
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical event hash anchoring this report's source.
    pub source_final_event_hash: String,
    /// Canonical event count anchoring this report's source.
    pub source_event_count: u64,
    /// Requested target selector.
    pub target: StatePoint,
    /// Resolved target semantic state.
    pub target_state: StateRecord,
    /// Inclusive event-sequence context radius.
    pub context_events: u64,
    /// Lossless tokens in the selected event context.
    pub tokens: Vec<VtToken>,
    /// Semantic transitions in the selected event context.
    pub states: Vec<StateRecord>,
    /// Conservative deterministic findings.
    pub findings: Vec<ExplainFinding>,
    /// Page coordinates when CLI output intentionally contains bounded subsets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_page: Option<ExplainOutputPage>,
}

/// Independent page coordinates for each ordered explain collection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainOutputPage {
    /// Lossless VT-token page.
    pub tokens: OutputPage,
    /// Semantic-state page.
    pub states: OutputPage,
    /// Deterministic-finding page.
    pub findings: OutputPage,
}

/// Stable detector selectable by `diagnose --check`.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCheck {
    /// Full-viewport semantic clear controls.
    FullRedraw,
    /// Ordered presented or hidden transient states.
    Transient,
    /// Semantic changes inconsistent with Ghostty dirty evidence.
    Dirty,
    /// Large cursor movement between semantic boundaries.
    Cursor,
    /// Repeated terminal-mode mutations.
    Modes,
    /// Repeated palette/default-color mutations.
    Palette,
    /// Repeated equal query/response exchanges.
    QueryLoop,
    /// Control strings the semantic engine ignored and processing errors it reported.
    Effects,
}

impl DiagnosticCheck {
    /// Complete deterministic detector inventory.
    #[must_use]
    pub fn all() -> Vec<Self> {
        return vec![
            Self::FullRedraw,
            Self::Transient,
            Self::Dirty,
            Self::Cursor,
            Self::Modes,
            Self::Palette,
            Self::QueryLoop,
            Self::Effects,
        ];
    }
}

/// Strength of evidence relating a semantic finding to native presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeEvidenceLevel {
    /// The semantic state was eligible for terminal presentation.
    PresentationEligible,
    /// Clock-map arithmetic assigned the state a nominal video frame.
    ScheduledForFrame,
    /// A stable native frame was captured after replay reached this endpoint.
    PostEndpointStableCapture,
    /// A causal presentation mechanism verified actual captured output.
    FrameObserved,
}

/// One source-bound relationship to a native artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeCorrelation {
    /// Honest evidence level.
    pub level: NativeEvidenceLevel,
    /// Relative native manifest path.
    #[serde(with = "crate::value::exact_path")]
    #[schemars(with = "crate::value::ExactPath")]
    pub manifest: PathBuf,
    /// Related semantic state.
    pub state_id: u64,
    /// Nominal encoded frame when clock mapping supplied one.
    pub frame: Option<u64>,
    /// Exact limitation on the relationship.
    pub limitation: String,
}

/// One conservative session-wide diagnostic finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Conservative confidence label.
    pub confidence: FindingConfidence,
    /// Supporting semantic state IDs.
    pub state_ids: Vec<u64>,
    /// Supporting VT token IDs.
    pub token_ids: Vec<u64>,
    /// Exact neutral facts and limitations.
    pub facts: Vec<String>,
    /// Native relationships, if independently available.
    pub native_correlations: Vec<NativeCorrelation>,
}

/// Deterministic report from conservative session-wide detectors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnoseReport {
    /// Stable report format.
    pub format: String,
    /// Report schema version.
    pub version: u32,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical event hash anchoring the report.
    pub source_final_event_hash: String,
    /// Canonical event count anchoring the report.
    pub source_event_count: u64,
    /// Ordered detector selection.
    pub checks: Vec<DiagnosticCheck>,
    /// Minimum changed-cell fraction used for broad redraw findings.
    pub broad_change_fraction: String,
    /// Deterministic findings.
    pub findings: Vec<DiagnosticFinding>,
    /// Page coordinates when CLI output intentionally contains a bounded subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_page: Option<OutputPage>,
}
