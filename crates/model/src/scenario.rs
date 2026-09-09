use crate::{
    execution::environment::{ColorPolicy, EnvironmentBase},
    terminal::state::NamedKey,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use uuid::Uuid;

/// Overall scenario disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    /// Every step completed successfully.
    Passed,
    /// One step failed and later steps did not run.
    Failed,
}

/// One executed step and its canonical provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioStepResult {
    /// Zero-based step index.
    pub index: usize,
    /// Stable scenario step kind.
    pub kind: String,
    /// Caller-owned mutation request identifiers used by this step.
    pub request_ids: Vec<Uuid>,
    /// Matched presented state hash when applicable.
    pub state_hash: Option<String>,
}

/// Stable failure context for one scenario step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioFailure {
    /// Zero-based failed step index.
    pub step_index: usize,
    /// Stable scenario step kind.
    pub step_kind: String,
    /// Caller-owned mutation identifiers allocated before the failure.
    pub request_ids: Vec<Uuid>,
    /// Safe diagnostic without child environment values.
    pub message: String,
}

/// Machine-readable scenario outcome.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioReport {
    /// Scenario format version.
    pub version: u32,
    /// Scenario name.
    pub name: String,
    /// Overall disposition.
    pub status: ScenarioStatus,
    /// Immutable canonical session identity.
    pub session_id: Uuid,
    /// Owner-only canonical evidence path.
    pub canonical_session: PathBuf,
    /// Successfully completed steps.
    pub steps: Vec<ScenarioStepResult>,
    /// First failure, if any.
    pub failure: Option<ScenarioFailure>,
    /// Atomic failure evidence directory, if the scenario failed.
    pub failure_artifacts: Option<PathBuf>,
}

/// One fresh-session attempt retained within a scenario suite result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioAttemptReport {
    /// One-based attempt number.
    pub attempt: u32,
    /// Complete fresh-session result.
    pub report: ScenarioReport,
}

/// Every attempt for one selected scenario document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioCaseReport {
    /// Input scenario document path.
    pub scenario: PathBuf,
    /// Ordered fresh-session attempts, including failed attempts before a pass.
    pub attempts: Vec<ScenarioAttemptReport>,
}

/// Overall multi-scenario disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioSuiteStatus {
    /// Every selected scenario eventually passed.
    Passed,
    /// At least one selected scenario exhausted its attempts without passing.
    Failed,
}

/// Machine-readable sharded, bounded-concurrency scenario suite result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSuiteReport {
    /// Suite result contract version.
    pub version: u32,
    /// Overall disposition.
    pub status: ScenarioSuiteStatus,
    /// Requested maximum worker count.
    pub jobs: usize,
    /// Optional zero-based shard index.
    pub shard_index: Option<usize>,
    /// Optional total shard count.
    pub shard_count: Option<usize>,
    /// Ordered selected scenarios and their retained attempts.
    pub cases: Vec<ScenarioCaseReport>,
}

/// Current scenario document version.
pub const SCENARIO_VERSION: u32 = 2;

/// One fresh-session terminal scenario.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Scenario schema version.
    #[schemars(range(equal = SCENARIO_VERSION))]
    pub version: u32,
    /// Stable human-readable scenario name.
    pub name: String,
    /// Fresh canonical session configuration.
    pub launch: ScenarioLaunch,
    /// Sequential operations and assertions.
    pub steps: Vec<ScenarioStep>,
    /// Optional bounded Cartesian substitutions for `{{key}}` tokens.
    #[serde(default)]
    pub matrix: BTreeMap<String, Vec<String>>,
}

/// Fresh canonical session launch configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioLaunch {
    /// Executable followed by exact argument strings.
    pub command: Vec<String>,
    /// Initial terminal grid.
    #[serde(default)]
    pub geometry: ScenarioGeometry,
    /// Optional child working directory.
    pub working_directory: Option<PathBuf>,
    /// Deterministic child environment.
    #[serde(default)]
    pub environment: ScenarioEnvironment,
    /// Soft output budget; all observed bytes remain canonical evidence.
    pub output_byte_budget: Option<u64>,
    /// Default deadline for waits and completion.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

/// Public scenario environment with unambiguous text and byte value encodings.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioEnvironment {
    /// Source environment used before overrides.
    #[serde(default)]
    pub base: EnvironmentBase,
    /// UTF-8 environment values.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// Arbitrary environment values encoded explicitly as Base64.
    #[serde(default)]
    pub set_bytes: BTreeMap<String, ScenarioEncodedBytes>,
    /// Names copied from the recorder environment.
    #[serde(default)]
    pub copy: BTreeSet<String>,
    /// Names removed from an inherited environment.
    #[serde(default)]
    pub unset: BTreeSet<String>,
    /// Deterministic color-variable behavior.
    #[serde(default)]
    pub color: ColorPolicy,
}

/// One arbitrary scenario environment value encoded as standard padded Base64.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEncodedBytes {
    /// Exact environment bytes.
    pub base64: ByteString,
}

/// Initial scenario terminal grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioGeometry {
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
}

impl Default for ScenarioGeometry {
    fn default() -> Self {
        return Self {
            cols: 160,
            rows: 45,
        };
    }
}

/// One sequential scenario operation or assertion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ScenarioStep {
    /// Wait on the latest presentation-eligible viewport.
    WaitScreen(ScenarioScreenWait),
    /// Apply one or more typed input atoms in order.
    Send(Vec<ScenarioInput>),
    /// Add a canonical marker.
    Mark(String),
    /// Compare the latest stable presented frame with a normalized baseline.
    MatchScreenSnapshot(ScenarioSnapshot),
    /// Assert temporal semantics and diagnostic findings between two markers.
    AssertTimeline(ScenarioTimelineAssertion),
    /// Gracefully stop the child and await completion.
    Stop {},
}

impl ScenarioStep {
    /// Stable operation name for reports.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WaitScreen(_) => return "wait_screen",
            Self::Send(_) => return "send",
            Self::Mark(_) => return "mark",
            Self::MatchScreenSnapshot(_) => return "match_screen_snapshot",
            Self::AssertTimeline(_) => return "assert_timeline",
            Self::Stop {} => return "stop",
        }
    }
}

/// Stable normalized presented-frame snapshot request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSnapshot {
    /// Filesystem-safe logical baseline name.
    pub name: String,
    /// Required unchanged-state interval before comparison.
    #[serde(default)]
    pub stable_for_ms: u64,
    /// Per-step deadline overriding the launch default.
    pub timeout_ms: Option<u64>,
}

/// Temporal assertions evaluated over states between two canonical markers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioTimelineAssertion {
    /// Exactly two unique marker labels defining the inclusive time range.
    pub between_markers: Vec<String>,
    /// Text that must not appear in any presentation-eligible state in the range.
    pub no_presented_screen_contains: Option<String>,
    /// Stable diagnose finding code that must not overlap the range.
    pub no_finding: Option<String>,
}

/// Volatility-free semantic frame baseline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioFrameSnapshot {
    /// Snapshot contract version.
    pub version: u32,
    /// Presentation-eligible viewport text.
    pub viewport_text: String,
    /// Exact viewport geometry.
    pub cols: u16,
    /// Exact viewport geometry.
    pub rows: u16,
    /// Cursor column when visible in the viewport.
    pub cursor_column: Option<u16>,
    /// Cursor row when visible in the viewport.
    pub cursor_row: Option<u16>,
    /// Cursor visibility.
    pub cursor_visible: bool,
}

/// Presented-screen predicate with an optional stability window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioScreenWait {
    /// Required exact visible substring.
    pub contains: Option<String>,
    /// Exact substring that must be absent.
    pub absent: Option<String>,
    /// Required duration for one unchanged matching state.
    #[serde(default)]
    pub stable_for_ms: u64,
    /// Per-step deadline overriding the launch default.
    pub timeout_ms: Option<u64>,
}

/// One semantic input atom.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ScenarioInput {
    /// Exact UTF-8 text.
    Text(ScenarioTextInput),
    /// One mode-aware named key.
    Key(ScenarioKeyInput),
}

/// Exact UTF-8 scenario input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioTextInput {
    /// Text bytes to apply.
    pub text: String,
}

/// Mode-aware scenario key input.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioKeyInput {
    /// Named key encoded against the actor's live modes.
    pub key: NamedKey,
}

fn default_timeout_ms() -> u64 {
    return 10_000;
}
