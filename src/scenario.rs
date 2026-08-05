//! Strict language-neutral scenario contracts and execution reports.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::Permissions,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zstd::stream::decode_all;

use crate::{
    Error, Result,
    analysis::{DiagnosticCheck, diagnose},
    client::{Session, Terminal},
    control::{ControlCommand, ControlData, ControlRequest, ScreenPredicate},
    environment::EnvironmentSpec,
    event::{ByteString, Event, InputKind},
    event_log::EventLog,
    ghostty_vt::{KeyModifiers, NamedKey, SemanticState},
    inspection::{state_text, wait_for_screen},
    replay::{StateSelector, inspect, replay},
    semantic_image::render_png,
    timeline::build_timeline,
};

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

impl Scenario {
    /// Parse and validate one strict scenario document.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let encoded = fs_err::read(path).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        let scenario: Self = serde_json::from_slice(&encoded).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        scenario.validate()?;
        return Ok(scenario);
    }

    fn validate(&self) -> Result<()> {
        if self.version != SCENARIO_VERSION {
            return Err(scenario_error(format!(
                "scenario version {} is unsupported; expected {SCENARIO_VERSION}",
                self.version
            )));
        }
        if self.name.trim().is_empty() || self.launch.command.is_empty() || self.steps.is_empty() {
            return Err(scenario_error(
                "scenario name, launch command, and steps must not be empty",
            ));
        }
        if self.launch.geometry.cols == 0 || self.launch.geometry.rows == 0 {
            return Err(scenario_error(
                "scenario geometry must be greater than zero",
            ));
        }
        if self.launch.timeout_ms == 0 || self.launch.output_byte_budget == Some(0) {
            return Err(scenario_error(
                "scenario timeout and configured resource budgets must be greater than zero",
            ));
        }
        self.launch.environment.validate()?;
        for step in &self.steps {
            if let ScenarioStep::WaitScreen(wait_screen) = step {
                if wait_screen.contains.as_deref().is_none_or(str::is_empty)
                    && wait_screen.absent.as_deref().is_none_or(str::is_empty)
                {
                    return Err(scenario_error(
                        "screen wait requires contains or absent text",
                    ));
                }
                if wait_screen.contains.is_some() == wait_screen.absent.is_some() {
                    return Err(scenario_error(
                        "screen wait requires exactly one of contains or absent",
                    ));
                }
            }
            if matches!(step, ScenarioStep::Send(inputs) if inputs.is_empty()) {
                return Err(scenario_error("send step must contain at least one input"));
            }
            if let ScenarioStep::MatchScreenSnapshot(snapshot) = step
                && snapshot.name.trim().is_empty()
            {
                return Err(scenario_error("screen snapshot name must not be empty"));
            }
            if let ScenarioStep::AssertTimeline(assertion) = step
                && assertion.between_markers.len() != 2
            {
                return Err(scenario_error(
                    "timeline assertion requires exactly two marker labels",
                ));
            }
        }
        let mut combinations = 1_usize;
        for (key, values) in &self.matrix {
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || values.is_empty()
                || values.len() > 64
            {
                return Err(scenario_error(
                    "matrix keys must be safe identifiers with 1 through 64 values",
                ));
            }
            combinations = combinations.saturating_mul(values.len());
            if combinations > 256 {
                return Err(scenario_error("scenario matrix exceeds 256 variants"));
            }
        }
        return Ok(());
    }

    /// Expand a bounded Cartesian matrix into independently named scenarios.
    pub fn expand_matrix(self) -> Result<Vec<(String, Self)>> {
        self.validate()?;
        if self.matrix.is_empty() {
            return Ok(vec![(String::new(), self)]);
        }
        let mut variants = vec![BTreeMap::<String, String>::new()];
        for (key, values) in &self.matrix {
            let mut expanded = Vec::new();
            for variant in &variants {
                for value in values {
                    let mut next = variant.clone();
                    next.insert(key.clone(), value.clone());
                    expanded.push(next);
                }
            }
            variants = expanded;
        }
        return variants
            .into_iter()
            .map(|values| {
                let label = values
                    .iter()
                    .map(|(key, value)| return format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let mut scenario = self.clone();
                scenario.matrix.clear();
                scenario.name = format!("{} [{label}]", scenario.name);
                for argument in &mut scenario.launch.command {
                    *argument = substitute_matrix(argument, &values)?;
                }
                for step in &mut scenario.steps {
                    if let ScenarioStep::MatchScreenSnapshot(snapshot) = step {
                        let suffix = blake3::hash(label.as_bytes())
                            .to_hex()
                            .chars()
                            .take(12)
                            .collect::<String>();
                        snapshot.name = format!("{}-{suffix}", snapshot.name);
                    }
                }
                return Ok((label, scenario));
            })
            .collect();
    }
}

fn substitute_matrix(value: &str, matrix: &BTreeMap<String, String>) -> Result<String> {
    let mut substituted = value.to_owned();
    for (key, replacement) in matrix {
        substituted = substituted.replace(&format!("{{{{{key}}}}}"), replacement);
    }
    if substituted.contains("{{") || substituted.contains("}}") {
        return Err(scenario_error(format!(
            "unresolved matrix token in command argument `{value}`"
        )));
    }
    return Ok(substituted);
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
    /// Deterministic child environment; values are redacted from failure artifacts.
    #[serde(default)]
    pub environment: EnvironmentSpec,
    /// Soft output budget; all observed bytes remain canonical evidence.
    pub output_byte_budget: Option<u64>,
    /// Default deadline for waits and completion.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
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
        return match self {
            Self::WaitScreen(_) => "wait_screen",
            Self::Send(_) => "send",
            Self::Mark(_) => "mark",
            Self::MatchScreenSnapshot(_) => "match_screen_snapshot",
            Self::AssertTimeline(_) => "assert_timeline",
            Self::Stop {} => "stop",
        };
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

/// Execute a scenario in a fresh canonical session.
pub fn run(
    scenario: &Scenario,
    binary: impl Into<PathBuf>,
    evidence_root: impl Into<PathBuf>,
    failure_root: impl Into<PathBuf>,
    update_snapshots: bool,
) -> Result<ScenarioReport> {
    scenario.validate()?;
    let failure_root = failure_root.into();
    let terminal = Terminal::new(binary, evidence_root);
    let command = scenario.launch.command.iter().map(OsString::from);
    let mut builder = terminal
        .launch(command)
        .geometry(scenario.launch.geometry.cols, scenario.launch.geometry.rows)
        .startup_timeout(Duration::from_millis(scenario.launch.timeout_ms))
        .environment(scenario.launch.environment.clone());
    if let Some(limit) = scenario.launch.output_byte_budget {
        builder = builder.output_byte_budget(limit);
    }
    if let Some(directory) = &scenario.launch.working_directory {
        builder = builder.working_directory(directory);
    }
    let mut session = builder.start()?;
    let mut steps = Vec::new();
    let mut stopped = false;
    for (index, step) in scenario.steps.iter().enumerate() {
        match execute_step(
            &mut session,
            scenario,
            index,
            step,
            &failure_root,
            update_snapshots,
        ) {
            Ok(result) => {
                stopped |= matches!(step, ScenarioStep::Stop {});
                steps.push(result);
            }
            Err(failure) => {
                let final_snapshot = session.screen().snapshot().ok();
                let _stop_result =
                    session.stop(Some(Duration::from_millis(scenario.launch.timeout_ms)));
                let failure_artifacts = failure_root.join(session.session_id().to_string());
                let report = ScenarioReport {
                    version: SCENARIO_VERSION,
                    name: scenario.name.clone(),
                    status: ScenarioStatus::Failed,
                    session_id: session.session_id(),
                    canonical_session: session.path().to_path_buf(),
                    steps,
                    failure: Some(failure),
                    failure_artifacts: Some(failure_artifacts.clone()),
                };
                publish_failure(
                    &failure_artifacts,
                    scenario,
                    &report,
                    final_snapshot.as_ref(),
                )?;
                return Ok(report);
            }
        }
    }
    if !stopped {
        session.stop(Some(Duration::from_millis(scenario.launch.timeout_ms)))?;
    }
    return Ok(ScenarioReport {
        version: SCENARIO_VERSION,
        name: scenario.name.clone(),
        status: ScenarioStatus::Passed,
        session_id: session.session_id(),
        canonical_session: session.path().to_path_buf(),
        steps,
        failure: None,
        failure_artifacts: None,
    });
}

fn execute_step(
    session: &mut Session,
    scenario: &Scenario,
    index: usize,
    step: &ScenarioStep,
    failure_root: &Path,
    update_snapshots: bool,
) -> StdResult<ScenarioStepResult, ScenarioFailure> {
    let mut request_ids = Vec::new();
    let outcome = match step {
        ScenarioStep::WaitScreen(wait) => {
            let predicate = match (&wait.contains, &wait.absent) {
                (Some(text), None) => ScreenPredicate::Contains { text: text.clone() },
                (None, Some(text)) => ScreenPredicate::Absent { text: text.clone() },
                _ => {
                    return Err(step_failure(
                        index,
                        step,
                        request_ids,
                        "invalid screen predicate",
                    ));
                }
            };
            return wait_for_screen(
                session.path(),
                predicate,
                Duration::from_millis(wait.stable_for_ms),
                Duration::from_millis(wait.timeout_ms.unwrap_or(scenario.launch.timeout_ms)),
            )
            .map(|matched| {
                return ScenarioStepResult {
                    index,
                    kind: step.kind().to_owned(),
                    request_ids,
                    state_hash: Some(matched.state_hash),
                };
            })
            .map_err(|error| {
                return step_failure(index, step, Vec::new(), error.to_string());
            });
        }
        ScenarioStep::Send(inputs) => {
            for input in inputs {
                let command = match input {
                    ScenarioInput::Text(input) => ControlCommand::Input {
                        data: ByteString::new(input.text.as_bytes().to_vec()),
                        input_kind: InputKind::Text,
                        intent: None,
                    },
                    ScenarioInput::Key(input) => ControlCommand::Key {
                        key: input.key,
                        modifiers: KeyModifiers::default(),
                    },
                };
                let request = ControlRequest::new(command);
                request_ids.push(request.request_id);
                if let Err(error) = session.send(&request) {
                    return Err(step_failure(index, step, request_ids, error.to_string()));
                }
            }
            Ok(None)
        }
        ScenarioStep::Mark(label) => {
            let request = ControlRequest::new(ControlCommand::Mark {
                label: label.clone(),
            });
            request_ids.push(request.request_id);
            session
                .send(&request)
                .map(|_data| return None)
                .map_err(|error| {
                    return step_failure(index, step, request_ids.clone(), error.to_string());
                })
        }
        ScenarioStep::MatchScreenSnapshot(snapshot) => {
            let capture = session
                .screen()
                .wait_for_idle(
                    Duration::from_millis(snapshot.stable_for_ms),
                    Duration::from_millis(
                        snapshot.timeout_ms.unwrap_or(scenario.launch.timeout_ms),
                    ),
                )
                .map_err(|error| {
                    return step_failure(index, step, Vec::new(), error.to_string());
                })?;
            let current = capture.snapshot;
            let ControlData::StableSnapshot { state_hash, .. } = &current else {
                return Err(step_failure(
                    index,
                    step,
                    Vec::new(),
                    "screen endpoint did not return an atomic stable snapshot",
                ));
            };
            let baseline = frame_snapshot(&current).map_err(|error| {
                return step_failure(index, step, Vec::new(), error.to_string());
            })?;
            let path = snapshot_path(failure_root, &snapshot.name).map_err(|error| {
                return step_failure(index, step, Vec::new(), error.to_string());
            })?;
            if update_snapshots {
                if let Some(parent) = path.parent() {
                    fs_err::create_dir_all(parent).map_err(|error| {
                        return step_failure(index, step, Vec::new(), error.to_string());
                    })?;
                }
                write_json(path, &baseline).map_err(|error| {
                    return step_failure(index, step, Vec::new(), error.to_string());
                })?;
            } else {
                let expected = fs_err::read(&path)
                    .map_err(|error| {
                        return step_failure(
                            index,
                            step,
                            Vec::new(),
                            format!("read snapshot {}: {error}", path.display()),
                        );
                    })
                    .and_then(|bytes| {
                        return serde_json::from_slice::<ScenarioFrameSnapshot>(&bytes).map_err(
                            |error| {
                                return step_failure(
                                    index,
                                    step,
                                    Vec::new(),
                                    format!("decode snapshot {}: {error}", path.display()),
                                );
                            },
                        );
                    })?;
                if expected != baseline {
                    return Err(step_failure(
                        index,
                        step,
                        Vec::new(),
                        format!(
                            "screen snapshot `{}` differs from {}",
                            snapshot.name,
                            path.display()
                        ),
                    ));
                }
            }
            return Ok(ScenarioStepResult {
                index,
                kind: step.kind().to_owned(),
                request_ids,
                state_hash: Some(state_hash.clone()),
            });
        }
        ScenarioStep::AssertTimeline(assertion) => {
            assert_timeline(session.path(), assertion).map_err(|error| {
                return step_failure(index, step, Vec::new(), error.to_string());
            })?;
            Ok(None)
        }
        ScenarioStep::Stop {} => {
            let request = ControlRequest::new(ControlCommand::Stop);
            request_ids.push(request.request_id);
            session
                .send(&request)
                .and_then(|_data| {
                    return session
                        .complete(Duration::from_millis(scenario.launch.timeout_ms))
                        .map(|_completion| return None);
                })
                .map_err(|error| {
                    return step_failure(index, step, request_ids.clone(), error.to_string());
                })
        }
    };
    return outcome.map(|state_hash| {
        return ScenarioStepResult {
            index,
            kind: step.kind().to_owned(),
            request_ids,
            state_hash,
        };
    });
}

fn step_failure(
    index: usize,
    step: &ScenarioStep,
    request_ids: Vec<Uuid>,
    message: impl Into<String>,
) -> ScenarioFailure {
    return ScenarioFailure {
        step_index: index,
        step_kind: step.kind().to_owned(),
        request_ids,
        message: message.into(),
    };
}

fn frame_snapshot(data: &ControlData) -> Result<ScenarioFrameSnapshot> {
    let (ControlData::Snapshot {
        viewport_text,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        ..
    }
    | ControlData::StableSnapshot {
        viewport_text,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        ..
    }) = data
    else {
        return Err(scenario_error("screen endpoint did not return a snapshot"));
    };
    return Ok(ScenarioFrameSnapshot {
        version: 1,
        viewport_text: viewport_text.clone(),
        cols: *cols,
        rows: *rows,
        cursor_column: *cursor_column,
        cursor_row: *cursor_row,
        cursor_visible: *cursor_visible,
    });
}

fn snapshot_path(failure_root: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(scenario_error(
            "snapshot names accept 1 through 128 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    let artifact_root = failure_root.parent().ok_or_else(|| {
        return scenario_error("scenario failure root has no artifact parent");
    })?;
    return Ok(artifact_root
        .join("snapshots")
        .join(format!("{name}.frame.json")));
}

fn assert_timeline(session: &Path, assertion: &ScenarioTimelineAssertion) -> Result<()> {
    let [start_label, end_label] = assertion.between_markers.as_slice() else {
        return Err(scenario_error(
            "timeline assertion requires exactly two marker labels",
        ));
    };
    let log = EventLog::read(session.join("events.jsonl"))?;
    let marker_time = |label: &str| -> Result<u64> {
        let matches = log
            .events
            .iter()
            .filter_map(|record| match &record.event {
                Event::Marker { label: value, .. } if value == label => {
                    return Some(record.monotonic_ns);
                }
                _ => return None,
            })
            .collect::<Vec<_>>();
        let [time] = matches.as_slice() else {
            return Err(scenario_error(format!(
                "timeline marker `{label}` must occur exactly once, found {}",
                matches.len()
            )));
        };
        return Ok(*time);
    };
    let start = marker_time(start_label)?;
    let end = marker_time(end_label)?;
    if end < start {
        return Err(scenario_error("timeline markers are in reverse order"));
    }
    let replayed = replay(&log)?;
    let ranged_states = replayed
        .states
        .iter()
        .filter(|state| return state.monotonic_ns >= start && state.monotonic_ns <= end)
        .collect::<Vec<_>>();
    if let Some(text) = &assertion.no_presented_screen_contains {
        for state in &ranged_states {
            if state.presented
                && state_text(&inspect(&log, StateSelector::StateId(state.state_id))?.state)
                    .contains(text)
            {
                return Err(scenario_error(format!(
                    "a presented state between `{start_label}` and `{end_label}` contained forbidden text"
                )));
            }
        }
    }
    if let Some(code) = &assertion.no_finding {
        let report = diagnose(session, &log, DiagnosticCheck::all(), 50)?;
        let state_ids = ranged_states
            .iter()
            .map(|state| return state.state_id)
            .collect::<BTreeSet<_>>();
        if report.findings.iter().any(|finding| {
            return finding.code == *code
                && (finding.state_ids.is_empty()
                    || finding
                        .state_ids
                        .iter()
                        .any(|state_id| return state_ids.contains(state_id)));
        }) {
            return Err(scenario_error(format!(
                "diagnostic finding `{code}` overlapped `{start_label}` through `{end_label}`"
            )));
        }
    }
    return Ok(());
}

fn publish_failure(
    destination: &Path,
    scenario: &Scenario,
    report: &ScenarioReport,
    final_snapshot: Option<&ControlData>,
) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        return scenario_error("scenario failure destination must have a parent directory");
    })?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let temporary = parent.join(format!(".{}.tmp", report.session_id));
    fs_err::create_dir(&temporary).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;
    fs_err::set_permissions(&temporary, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;
    let mut redacted_scenario = scenario.clone();
    for value in redacted_scenario.launch.environment.set.values_mut() {
        *value = ByteString::from(b"<redacted>".as_slice());
    }
    write_json(temporary.join("scenario.json"), &redacted_scenario)?;
    write_json(temporary.join("result.json"), report)?;
    let mut derivation_errors = Vec::new();
    if let Some(snapshot) = final_snapshot {
        write_json(temporary.join("final-screen.json"), snapshot)?;
        if let ControlData::Snapshot {
            semantic_state_zstd,
            viewport_text,
            ..
        } = snapshot
        {
            write_bytes(temporary.join("final-screen.txt"), viewport_text.as_bytes())?;
            match decode_all(semantic_state_zstd.as_bytes())
                .map_err(|error| return error.to_string())
                .and_then(|json| {
                    return serde_json::from_slice::<SemanticState>(&json)
                        .map_err(|error| return error.to_string());
                })
                .and_then(|state| {
                    return render_png(&state).map_err(|error| return error.to_string());
                }) {
                Ok(png) => write_bytes(temporary.join("final-screen.png"), &png)?,
                Err(error) => derivation_errors.push(format!("semantic image: {error}")),
            }
        }
    }
    match EventLog::read(report.canonical_session.join("events.jsonl")) {
        Ok(log) => {
            match build_timeline(&log, true, true, None) {
                Ok(timeline) => write_json(temporary.join("timeline.json"), &timeline)?,
                Err(error) => derivation_errors.push(format!("timeline: {error}")),
            }
            match diagnose(&report.canonical_session, &log, DiagnosticCheck::all(), 50) {
                Ok(diagnosis) => write_json(temporary.join("diagnose.json"), &diagnosis)?,
                Err(error) => derivation_errors.push(format!("diagnose: {error}")),
            }
        }
        Err(error) => derivation_errors.push(format!("event log: {error}")),
    }
    if !derivation_errors.is_empty() {
        write_json(temporary.join("derivation-errors.json"), &derivation_errors)?;
    }
    fs_err::rename(&temporary, destination).map_err(|source| {
        return Error::Filesystem {
            path: destination.to_path_buf(),
            source,
        };
    })?;
    return Ok(());
}

fn write_bytes(path: PathBuf, bytes: &[u8]) -> Result<()> {
    return fs_err::write(&path, bytes).map_err(|source| {
        return Error::Filesystem { path, source };
    });
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    encoded.push(b'\n');
    return fs_err::write(&path, encoded).map_err(|source| {
        return Error::Filesystem { path, source };
    });
}

fn default_timeout_ms() -> u64 {
    return 10_000;
}

fn scenario_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_scenario_parses_typed_steps() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "ready flow",
            "launch": { "command": ["/bin/sh"], "geometry": { "cols": 90, "rows": 25 } },
            "steps": [
                { "waitScreen": { "contains": "Ready", "stableForMs": 20 } },
                { "send": [{ "text": "help" }, { "key": "enter" }] },
                { "mark": "help-visible" },
                { "stop": {} }
            ]
        }))
        .expect("scenario JSON");
        scenario.validate().expect("valid scenario");
        assert_eq!(scenario.steps[1].kind(), "send");
    }

    #[test]
    fn scenario_version_one_is_rejected() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 1,
            "name": "obsolete",
            "launch": { "command": ["/bin/true"] },
            "steps": [{ "stop": {} }]
        }))
        .expect("version one scenario JSON");
        assert!(scenario.validate().is_err());
    }

    #[test]
    fn scenario_rejects_ambiguous_screen_predicate() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "ambiguous",
            "launch": { "command": ["/bin/true"] },
            "steps": [{ "waitScreen": { "contains": "yes", "absent": "no" } }]
        }))
        .expect("scenario JSON");
        assert!(scenario.validate().is_err());
    }

    #[test]
    fn scenario_matrix_expands_commands_and_isolates_snapshot_names() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "matrix",
            "matrix": { "theme": ["dark", "light"], "width": ["80", "120"] },
            "launch": { "command": ["/bin/echo", "{{theme}}", "{{width}}"] },
            "steps": [{ "matchScreenSnapshot": { "name": "matrix-final" } }]
        }))
        .expect("scenario JSON");
        let expanded = scenario.expand_matrix().expect("matrix expansion");
        assert_eq!(expanded.len(), 4);
        let first = expanded.first().expect("first matrix variant");
        assert_eq!(first.1.launch.command.get(1).expect("theme"), "dark");
        assert_eq!(first.1.launch.command.get(2).expect("width"), "80");
        let names = expanded
            .iter()
            .filter_map(|(_label, scenario)| {
                return match scenario.steps.first() {
                    Some(ScenarioStep::MatchScreenSnapshot(snapshot)) => {
                        Some(snapshot.name.clone())
                    }
                    _ => None,
                };
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), 4);
    }
}
