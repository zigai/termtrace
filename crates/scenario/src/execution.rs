use crate::{
    artifact::{publish_failure, snapshot_path, write_json},
    document::{to_environment_spec, validate},
    error::{invalid, operation},
    Error, Result,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::ErrorKind,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    result::Result as StdResult,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use termtrace_engine::{
    analysis::diagnostics::diagnose_snapshot,
    observation::{observe_session_with_options, state_text, StatePoint},
    replay::visit_snapshot_states_while,
    timeline::build_session_timeline_page,
};
use termtrace_evidence::{
    event_log::EventLogCursor,
    session::{validate_completed_session, SessionSnapshot},
};
use termtrace_model::scenario::{
    Scenario, ScenarioFailure, ScenarioFrameSnapshot, ScenarioInfrastructureError, ScenarioInput,
    ScenarioReport, ScenarioStatus, ScenarioStep, ScenarioStepResult, ScenarioTimelineAssertion,
    SCENARIO_REPORT_VERSION,
};
use termtrace_model::{
    execution::{event::Event, layout::SessionLayout},
    protocol::control::{ControlCommand, ControlPayload, ControlRequest, ScreenPredicate},
    terminal::analysis::DiagnosticCheck,
    terminal::screen::{ScreenFrame, ScreenFrameContent},
    terminal::state::{KeyChord, KeyModifiers},
};
use termtrace_service::{
    ActRequest, InputAtom, LaunchSpec, MutationGuard, Session, StableCaptureOptions, StopRequest,
    Terminal,
};
use uuid::Uuid;

/// Execute a scenario in a fresh canonical session.
///
/// # Errors
///
/// Returns an error for document/preflight failures. Executed expectation and infrastructure
/// outcomes (including launch failures) are retained in the returned versioned report.
pub fn run(
    scenario: &Scenario,
    binary: impl Into<PathBuf>,
    evidence_root: impl Into<PathBuf>,
    failure_root: impl Into<PathBuf>,
    update_snapshots: bool,
) -> Result<ScenarioReport> {
    validate(scenario)?;

    return run_validated(
        scenario,
        binary,
        evidence_root,
        failure_root,
        update_snapshots,
    );
}

pub(crate) fn run_validated(
    scenario: &Scenario,
    binary: impl Into<PathBuf>,
    evidence_root: impl Into<PathBuf>,
    failure_root: impl Into<PathBuf>,
    update_snapshots: bool,
) -> Result<ScenarioReport> {
    let failure_root = failure_root.into();
    let terminal = Terminal::new(binary, evidence_root);
    let command = scenario.launch.command.iter().map(OsString::from);

    let mut spec = LaunchSpec::new(command)
        .geometry(scenario.launch.geometry.cols, scenario.launch.geometry.rows)
        .startup_timeout(Duration::from_millis(scenario.launch.timeout_ms))
        .environment(to_environment_spec(&scenario.launch.environment)?);
    spec.output_byte_budget = scenario.launch.output_byte_budget;

    spec.working_directory
        .clone_from(&scenario.launch.working_directory);
    let session = match terminal.start(spec) {
        Ok(session) => session,
        Err(error) => return Ok(error_report(&scenario.name, "launch", error.code())),
    };

    // A worker panic must not abandon its recorder/process group. Ordinary errors use the
    // bounded graceful lifecycle below and retain diagnostics; unwinding gets force-close.
    let mut session = OwnedSession(session);
    let timeout = Duration::from_millis(scenario.launch.timeout_ms);
    let mut report = ScenarioReport {
        version: SCENARIO_REPORT_VERSION,
        name: scenario.name.clone(),
        status: ScenarioStatus::Passed,
        session_id: Some(session.session_id()),
        canonical_session: Some(session.path().to_path_buf()),
        steps: Vec::new(),
        failure: None,
        failure_artifacts: None,
        infrastructure_errors: Vec::new(),
        artifact_warnings: Vec::new(),
    };

    if let Some(error) = completion_failure(&session) {
        report.infrastructure_errors.push(error);
    }

    let mut stopped = false;
    let mut final_snapshot = None;

    for (index, step) in scenario.steps.iter().enumerate() {
        if !report.infrastructure_errors.is_empty() {
            break;
        }

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
                report.steps.push(result);
            }
            Err(error) => {
                report.failure = Some(error.failure);

                if let Some(infrastructure) = error.infrastructure {
                    report.infrastructure_errors.push(*infrastructure);
                }

                match session.control(&ControlRequest::new(ControlCommand::Snapshot)) {
                    Ok(snapshot) => final_snapshot = Some(snapshot),
                    Err(_) => report
                        .artifact_warnings
                        .push("final screen unavailable".to_owned()),
                }

                break;
            }
        }
    }

    if !stopped {
        finalize_session(&mut session.0, timeout, &mut report.infrastructure_errors);
    }

    if let Some(error) = completion_failure(&session)
        && !report.infrastructure_errors.contains(&error)
    {
        report.infrastructure_errors.push(error);
    }

    if let Err(_error) = validate_replay_closure(session.path(), &report.steps) {
        report
            .infrastructure_errors
            .push(infrastructure("replay", "replay_integrity"));
    }

    report.status = if !report.infrastructure_errors.is_empty() {
        ScenarioStatus::Error
    } else if report.failure.is_some() {
        ScenarioStatus::Failed
    } else {
        ScenarioStatus::Passed
    };

    if report.status != ScenarioStatus::Passed {
        let destination = failure_root.join(session.session_id().to_string());
        publish_report_failure(destination, scenario, &mut report, final_snapshot.as_ref());
    }

    return Ok(report);
}

fn publish_report_failure(
    destination: PathBuf,
    scenario: &Scenario,
    report: &mut ScenarioReport,
    final_snapshot: Option<&ControlPayload>,
) {
    match publish_failure(&destination, scenario, report, final_snapshot) {
        Ok(warnings) => {
            report.failure_artifacts = Some(destination);
            report.artifact_warnings.extend(warnings);
        }
        Err(_) => {
            report
                .infrastructure_errors
                .push(infrastructure("publication", "artifact_write"));

            report.status = ScenarioStatus::Error;
        }
    }
}

fn completion_failure(session: &Session) -> Option<ScenarioInfrastructureError> {
    match session.completion() {
        Ok(Some(completion)) => {
            return completion.failure.map(|failure| {
                if failure.code == "spawn_failed" {
                    return infrastructure("launch", "spawn_failed");
                }
                return infrastructure("completion", "recorder_failure");
            })
        }
        Ok(None) => return None,
        Err(error) => return Some(infrastructure("completion", error.code())),
    }
}

struct OwnedSession(Session);

impl Deref for OwnedSession {
    type Target = Session;

    fn deref(&self) -> &Session {
        return &self.0;
    }
}

impl DerefMut for OwnedSession {
    fn deref_mut(&mut self) -> &mut Session {
        return &mut self.0;
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        let _closed = self.0.force_close();
    }
}

pub(crate) fn infrastructure(phase: &str, code: &str) -> ScenarioInfrastructureError {
    return ScenarioInfrastructureError {
        phase: phase.to_owned(),
        code: code.to_owned(),
        // Never stringify lower-layer errors: they can contain child launch values or output.
        message: format!("scenario {phase} failed ({code}); inspect retained canonical evidence"),
    };
}

pub(crate) fn error_report(name: &str, phase: &str, code: &str) -> ScenarioReport {
    return ScenarioReport {
        version: SCENARIO_REPORT_VERSION,
        name: name.to_owned(),
        status: ScenarioStatus::Error,
        session_id: None,
        canonical_session: None,
        steps: Vec::new(),
        failure: None,
        failure_artifacts: None,
        infrastructure_errors: vec![infrastructure(phase, code)],
        artifact_warnings: Vec::new(),
    };
}

trait Lifecycle {
    fn stop(&mut self, timeout: Duration) -> termtrace_service::Result<()>;
    fn complete(&mut self, timeout: Duration) -> termtrace_service::Result<()>;
    fn force_close(&mut self) -> termtrace_service::Result<()>;
}

impl Lifecycle for Session {
    fn stop(&mut self, timeout: Duration) -> termtrace_service::Result<()> {
        return Session::stop(self, StopRequest::default(), Some(timeout)).map(|_| ());
    }

    fn complete(&mut self, timeout: Duration) -> termtrace_service::Result<()> {
        return Session::complete(self, timeout).map(|_| ());
    }

    fn force_close(&mut self) -> termtrace_service::Result<()> {
        return Session::force_close(self);
    }
}

fn finalize_session(
    session: &mut impl Lifecycle,
    timeout: Duration,
    errors: &mut Vec<ScenarioInfrastructureError>,
) {
    let started = Instant::now();

    if let Err(error) = session.stop(timeout) {
        errors.push(infrastructure("stop", error.code()));

        if let Err(error) = session.complete(timeout.saturating_sub(started.elapsed())) {
            errors.push(infrastructure("completion", error.code()));

            if let Err(error) = session.force_close() {
                errors.push(infrastructure("force_close", error.code()));
            }
        }
    }
}

struct StepError {
    failure: ScenarioFailure,
    infrastructure: Option<Box<ScenarioInfrastructureError>>,
}

fn service_failure(
    index: usize,
    step: &ScenarioStep,
    ids: Vec<Uuid>,
    error: &termtrace_service::Error,
) -> StepError {
    if matches!(
        error,
        termtrace_service::Error::StableCaptureIncomplete { .. }
            | termtrace_service::Error::PostconditionIncomplete { .. }
    ) {
        return step_failure(
            index,
            step,
            ids,
            "requested screen condition did not complete",
        );
    }
    return infrastructure_step(index, step, ids, error.code());
}

fn infrastructure_step(index: usize, step: &ScenarioStep, ids: Vec<Uuid>, code: &str) -> StepError {
    return StepError {
        failure: ScenarioFailure {
            step_index: index,
            step_kind: step.kind().to_owned(),
            request_ids: ids,
            message: format!("scenario step infrastructure error ({code})"),
        },
        infrastructure: Some(Box::new(infrastructure(step.kind(), code))),
    };
}

fn validate_replay_closure(session: &Path, steps: &[ScenarioStepResult]) -> Result<()> {
    let completion = validate_completed_session(session)
        .map_err(|error| return operation("validate replay closure", error))?;

    let asserted_hashes: BTreeSet<&String> = steps
        .iter()
        .filter_map(|step| return step.state_hash.as_ref())
        .collect();

    for state_hash in asserted_hashes {
        let observation = observe_session_with_options(
            session,
            StatePoint::StateHash {
                state_hash: state_hash.clone(),
            },
            true,
        )
        .map_err(|error| return operation("observe asserted replay state", error))?;
        if observation.state_hash != *state_hash {
            return Err(invalid(format!(
                "replay closure resolved asserted state hash `{state_hash}` as `{}`",
                observation.state_hash
            )));
        }
    }

    let final_point = completion.event_count.checked_sub(1).map_or(
        StatePoint::StateId { state_id: 0 },
        |event_seq| {
            return StatePoint::EventSeq { event_seq };
        },
    );

    observe_session_with_options(session, final_point, true)
        .map_err(|error| return operation("observe final replay state", error))?;

    let page = build_session_timeline_page(session, true, true, None, 0, 64)
        .map_err(|error| return operation("build replay closure timeline", error))?;

    if page.entries.is_empty() {
        return Err(invalid("replay closure produced an empty timeline page"));
    }
    return Ok(());
}

fn execute_step(
    session: &mut Session,
    scenario: &Scenario,
    index: usize,
    step: &ScenarioStep,
    failure_root: &Path,
    update_snapshots: bool,
) -> StdResult<ScenarioStepResult, StepError> {
    let mut request_ids: Vec<Uuid> = Vec::new();
    let state_hash = match step {
        ScenarioStep::WaitScreen(wait) => {
            let predicate = match (&wait.contains, &wait.absent) {
                (Some(text), None) => ScreenPredicate::Contains { text: text.clone() },
                (None, Some(text)) => ScreenPredicate::Absent { text: text.clone() },
                _ => {
                    return Err(step_failure(
                        index,
                        step,
                        Vec::new(),
                        "scenario wait step requires exactly one of contains or absent".to_owned(),
                    ));
                }
            };

            let matched = session
                .wait_screen(
                    predicate,
                    Duration::from_millis(wait.stable_for_ms),
                    Duration::from_millis(wait.timeout_ms.unwrap_or(scenario.launch.timeout_ms)),
                )
                .map_err(|error| return service_failure(index, step, Vec::new(), &error))?;

            if !matched.is_satisfied() {
                return Err(step_failure(
                    index,
                    step,
                    request_ids,
                    format!(
                        "screen predicate did not meet the requested stability before {:?}",
                        matched.quality
                    ),
                ));
            }
            return Ok(ScenarioStepResult {
                index,
                kind: step.kind().to_owned(),
                request_ids,
                state_hash: Some(matched.frame.state_hash),
            });
        }

        ScenarioStep::Send(inputs) => {
            let request_id = Uuid::now_v7();
            request_ids.push(request_id);

            let atoms = inputs
                .iter()
                .map(|input| {
                    return match input {
                        ScenarioInput::Text(input) => InputAtom::Text(input.text.clone()),
                        ScenarioInput::Key(input) => InputAtom::Key(KeyChord {
                            key: input.key,
                            modifiers: KeyModifiers::default(),
                        }),
                    };
                })
                .collect();

            session
                .act(ActRequest {
                    atoms,
                    guard: MutationGuard {
                        request_id: Some(request_id),
                        if_state: None,
                    },
                    ..ActRequest::default()
                })
                .map_err(|error| {
                    return service_failure(index, step, request_ids.clone(), &error);
                })?;

            None
        }

        ScenarioStep::Mark(label) => {
            let request = ControlRequest::new(ControlCommand::Mark {
                label: label.clone(),
            });
            request_ids.push(request.request_id);
            session.control(&request).map_err(|error| {
                return service_failure(index, step, request_ids.clone(), &error);
            })?;
            None
        }

        ScenarioStep::MatchScreenSnapshot(snapshot) => {
            let capture = session
                .capture_stable(StableCaptureOptions {
                    stable_for: Duration::from_millis(snapshot.stable_for_ms),
                    timeout: Duration::from_millis(
                        snapshot.timeout_ms.unwrap_or(scenario.launch.timeout_ms),
                    ),
                    projection: Default::default(),
                    allow_incomplete: false,
                    reaction: None,
                })
                .map_err(|error| return service_failure(index, step, Vec::new(), &error))?;

            let state_hash = capture.frame.state_hash.clone();
            let baseline = rich_frame_snapshot(&capture.frame);
            let path = snapshot_path(failure_root, &snapshot.name).map_err(|_error| {
                return infrastructure_step(index, step, Vec::new(), "snapshot_storage");
            })?;

            if update_snapshots {
                if let Some(parent) = path.parent() {
                    fs_err::create_dir_all(parent).map_err(|_error| {
                        return infrastructure_step(index, step, Vec::new(), "snapshot_storage");
                    })?;
                }

                write_json(path, &baseline).map_err(|_error| {
                    return infrastructure_step(index, step, Vec::new(), "snapshot_storage");
                })?;
            } else {
                let bytes = fs_err::read(&path).map_err(|error| {
                    return if error.kind() == ErrorKind::NotFound {
                        step_failure(
                            index,
                            step,
                            Vec::new(),
                            format!("missing snapshot baseline {}", path.display()),
                        )
                    } else {
                        infrastructure_step(index, step, Vec::new(), "snapshot_read")
                    };
                })?;

                let expected: ScenarioFrameSnapshot =
                    serde_json::from_slice(&bytes).map_err(|error| {
                        return step_failure(
                            index,
                            step,
                            Vec::new(),
                            format!("decode snapshot {}: {error}", path.display()),
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
                state_hash: Some(state_hash),
            });
        }

        ScenarioStep::AssertTimeline(assertion) => {
            assert_timeline(session.path(), assertion).map_err(|error| {
                return match error {
                    Error::InvalidScenario { reason } => {
                        step_failure(index, step, Vec::new(), reason)
                    }
                    _ => infrastructure_step(index, step, Vec::new(), "timeline_replay"),
                };
            })?;
            None
        }

        ScenarioStep::Stop {} => {
            let request_id = Uuid::now_v7();
            request_ids.push(request_id);

            session
                .stop(
                    StopRequest {
                        guard: MutationGuard {
                            request_id: Some(request_id),
                            if_state: None,
                        },
                    },
                    Some(Duration::from_millis(scenario.launch.timeout_ms)),
                )
                .map_err(|error| {
                    return service_failure(index, step, request_ids.clone(), &error);
                })?;

            None
        }
    };

    return Ok(ScenarioStepResult {
        index,
        kind: step.kind().to_owned(),
        request_ids,
        state_hash,
    });
}

fn step_failure(
    index: usize,
    step: &ScenarioStep,
    request_ids: Vec<Uuid>,
    message: impl Into<String>,
) -> StepError {
    return StepError {
        infrastructure: None,
        failure: ScenarioFailure {
            step_index: index,
            step_kind: step.kind().to_owned(),
            request_ids,
            message: message.into(),
        },
    };
}

fn rich_frame_snapshot(frame: &ScreenFrame) -> ScenarioFrameSnapshot {
    let viewport_text = match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameContent::None => Vec::new(),
    }
    .join("\n");

    return ScenarioFrameSnapshot {
        version: 1,
        viewport_text,
        cols: frame.geometry.cols,
        rows: frame.geometry.rows,
        cursor_column: frame.cursor.column,
        cursor_row: frame.cursor.row,
        cursor_visible: frame.cursor.visible,
    };
}

fn assert_timeline(session: &Path, assertion: &ScenarioTimelineAssertion) -> Result<()> {
    let [start_label, end_label] = match assertion.between_markers.as_slice() {
        [start, end] => [start.as_str(), end.as_str()],
        _ => {
            return Err(operation(
                "assert timeline",
                "scenario timeline assertion requires exactly two marker labels",
            ));
        }
    };

    let snapshot = SessionSnapshot::capture(session, None)
        .map_err(|error| return operation("snapshot timeline assertion", error))?;

    let marker_time = |label: &str| -> Result<u64> {
        let mut cursor = EventLogCursor::open(SessionLayout::new(snapshot.path()).event_log())
            .map_err(|error| return operation("read timeline assertion", error))?;

        let mut count = 0_u64;
        let mut time = 0;

        while let Some(record) = cursor
            .next_record()
            .map_err(|error| return operation("read timeline assertion", error))?
        {
            if matches!(record.event, Event::Marker { label: value, .. } if value == label) {
                count = count.saturating_add(1);
                time = record.monotonic_ns;
            }
        }

        if count != 1 {
            return Err(invalid(format!(
                "timeline marker `{label}` must occur exactly once, found {count}"
            )));
        }
        return Ok(time);
    };
    let start = marker_time(start_label)?;
    let end = marker_time(end_label)?;

    if end < start {
        return Err(invalid("timeline markers are in reverse order"));
    }

    let mut state_bounds = None::<(u64, u64)>;
    let mut forbidden = false;

    visit_snapshot_states_while(&snapshot, None, &AtomicBool::new(false), |record, state| {
        if record.monotonic_ns >= start && record.monotonic_ns <= end {
            if let Some((_, last)) = &mut state_bounds {
                *last = record.state_id;
            } else {
                state_bounds = Some((record.state_id, record.state_id));
            }

            if record.presented
                && assertion
                    .no_presented_screen_contains
                    .as_ref()
                    .is_some_and(|text| return state_text(state).contains(text))
            {
                forbidden = true;
            }
        }

        return Ok(true);
    })
    .map_err(|error| return operation("replay timeline assertion", error))?;

    if forbidden {
        return Err(invalid(format!(
            "a presented state between `{start_label}` and `{end_label}` contained forbidden text"
        )));
    }

    if let Some(code) = &assertion.no_finding {
        let report = diagnose_snapshot(&snapshot, DiagnosticCheck::all(), 50)
            .map_err(|error| return operation("diagnose timeline assertion", error))?;

        if report.findings.iter().any(|finding| {
            return finding.code == *code
                && (finding.state_ids.is_empty()
                    || finding.state_ids.iter().any(|state_id| {
                        return state_bounds.is_some_and(|(first, last)| {
                            return *state_id >= first && *state_id <= last;
                        });
                    }));
        }) {
            return Err(invalid(format!(
                "diagnostic finding `{code}` overlapped `{start_label}` through `{end_label}`"
            )));
        }
    }

    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite::scenario_junit;
    use std::io::Error as IoError;
    use termtrace_model::protocol::mutation::MutationEffect;

    struct FakeLifecycle {
        fail: [bool; 3],
        calls: Vec<&'static str>,
        complete_timeout: Option<Duration>,
    }

    impl FakeLifecycle {
        fn call(&mut self, i: usize, name: &'static str) -> termtrace_service::Result<()> {
            self.calls.push(name);

            if self.fail[i] {
                return Err(termtrace_service::Error::InvalidOperation {
                    reason: "private child data".into(),
                    source: None,
                });
            }
            return Ok(());
        }
    }

    impl Lifecycle for FakeLifecycle {
        fn stop(&mut self, _: Duration) -> termtrace_service::Result<()> {
            return self.call(0, "stop");
        }

        fn complete(&mut self, timeout: Duration) -> termtrace_service::Result<()> {
            self.complete_timeout = Some(timeout);

            return self.call(1, "completion");
        }

        fn force_close(&mut self) -> termtrace_service::Result<()> {
            return self.call(2, "force_close");
        }
    }

    #[test]
    fn cleanup_preserves_order_and_primary_and_uses_force_close_fallback() {
        for (fail, expected) in [
            ([false, false, false], vec![]),
            ([true, false, false], vec!["stop"]),
            ([true, true, false], vec!["stop", "completion"]),
            (
                [true, true, true],
                vec!["stop", "completion", "force_close"],
            ),
        ] {
            let mut lifecycle = FakeLifecycle {
                fail,
                calls: vec![],
                complete_timeout: None,
            };

            let mut report = error_report("cleanup", "wait_screen", "control_transport");
            report.session_id = Some(Uuid::nil());
            report.canonical_session = Some(PathBuf::from("/canonical.tt"));
            report.failure = Some(ScenarioFailure {
                step_index: 2,
                step_kind: "wait_screen".into(),
                request_ids: vec![Uuid::nil()],
                message: "primary expectation failure".into(),
            });

            finalize_session(
                &mut lifecycle,
                Duration::from_secs(1),
                &mut report.infrastructure_errors,
            );
            assert!(lifecycle
                .complete_timeout
                .is_none_or(|timeout| return timeout <= Duration::from_secs(1)));
            assert_eq!(
                report
                    .infrastructure_errors
                    .iter()
                    .skip(1)
                    .map(|e| return e.phase.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                report.failure.as_ref().unwrap().message,
                "primary expectation failure"
            );
            assert_eq!(
                lifecycle.calls.len(),
                if !fail[0] {
                    1
                } else if !fail[1] {
                    2
                } else {
                    3
                }
            );
            assert!(!serde_json::to_string(&report)
                .unwrap()
                .contains("private child data"));
            use termtrace_model::scenario::{
                ScenarioAttemptReport, ScenarioCaseReport, ScenarioCaseStatus, ScenarioSuiteReport,
                ScenarioSuiteStatus, SCENARIO_SUITE_REPORT_VERSION,
            };
            let case = ScenarioCaseReport {
                scenario: "cleanup.json".into(),
                status: ScenarioCaseStatus::Failed,
                attempts: vec![ScenarioAttemptReport {
                    attempt: 1,
                    duration_ms: 10,
                    report,
                }],
            };
            assert_eq!(case.verdict(), Ok(ScenarioCaseStatus::Failed));
            let suite = ScenarioSuiteReport {
                version: SCENARIO_SUITE_REPORT_VERSION,
                status: ScenarioSuiteStatus::Failed,
                duration_ms: 10,
                fail_on_flaky: false,
                infrastructure_errors: vec![],
                jobs: 1,
                shard_index: None,
                shard_count: None,
                cases: vec![case],
            };
            assert!(suite.ci_failed());
            let xml = scenario_junit(&suite);
            assert!(xml.contains("primary expectation failure"));
            assert!(xml.contains("errors=\"1\""));
            assert!(xml.contains("failures=\"0\""));
        }
    }

    #[test]
    fn publication_appends_warnings_after_existing_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2, "name": "publication warnings",
            "launch": { "command": ["/bin/true"] }, "steps": []
        }))
        .unwrap();

        let mut report = error_report(&scenario.name, "replay", "replay_integrity");
        let id = Uuid::now_v7();
        report.session_id = Some(id);
        report.canonical_session = Some(temp.path().join("missing.tt"));
        report
            .artifact_warnings
            .push("final screen unavailable".to_owned());

        let destination = temp.path().join(id.to_string());

        // Exercise the same publication/merge path as run_validated. The missing
        // canonical fixture produces a new, optional publication warning.
        publish_report_failure(destination.clone(), &scenario, &mut report, None);

        assert_eq!(
            report.artifact_warnings,
            vec!["final screen unavailable", "event log unavailable"]
        );
        assert_eq!(report.failure_artifacts, Some(destination.clone()));
        assert_eq!(report.infrastructure_errors.len(), 1);

        let published: ScenarioReport =
            serde_json::from_slice(&fs_err::read(destination.join("result.json")).unwrap())
                .unwrap();
        assert_eq!(
            published, report,
            "published and returned reports must agree without duplicated warnings"
        );
        let diagnostics: Vec<String> = serde_json::from_slice(
            &fs_err::read(destination.join("derivation-errors.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(diagnostics, report.artifact_warnings);
    }

    #[test]
    fn typed_step_errors_and_replay_integrity_are_infrastructure() {
        let service_error =
            termtrace_service::Error::Control(termtrace_service::ControlError::Transport {
                session: "/canonical.tt".into(),
                step: "receive",
                effect: MutationEffect::NotApplied,
                source: IoError::other("private child data"),
            });

        let error = service_failure(2, &ScenarioStep::Stop {}, vec![Uuid::nil()], &service_error);
        assert_eq!(error.infrastructure.unwrap().phase, "stop");
        assert_eq!(error.failure.request_ids, vec![Uuid::nil()]);
        assert!(!error.failure.message.contains("private child data"));

        let temp = tempfile::tempdir().unwrap();
        assert!(validate_replay_closure(temp.path(), &[]).is_err());
    }
}
