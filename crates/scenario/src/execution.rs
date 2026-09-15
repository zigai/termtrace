use crate::{
    artifact::{publish_failure, snapshot_path, write_json},
    document::{to_environment_spec, validate},
    error::{invalid, operation},
    Result,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    result::Result as StdResult,
    sync::atomic::AtomicBool,
    time::Duration,
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
    Scenario, ScenarioFailure, ScenarioFrameSnapshot, ScenarioInput, ScenarioReport,
    ScenarioStatus, ScenarioStep, ScenarioStepResult, ScenarioTimelineAssertion, SCENARIO_VERSION,
};
use termtrace_model::{
    execution::event::{Event, InputKind},
    protocol::control::{ControlCommand, ControlRequest, ScreenPredicate, ScreenWaitQuality},
    terminal::analysis::DiagnosticCheck,
    terminal::screen::{ScreenFrame, ScreenFrameData},
    terminal::state::KeyModifiers,
    value::ByteString,
};
use termtrace_service::{
    client::Session, send_request, Error as SdkError, StableCaptureOptions, Terminal,
};
use uuid::Uuid;

/// Execute a scenario in a fresh canonical session.
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

    let mut builder = terminal
        .launch(command)
        .geometry(scenario.launch.geometry.cols, scenario.launch.geometry.rows)
        .startup_timeout(Duration::from_millis(scenario.launch.timeout_ms))
        .environment(to_environment_spec(&scenario.launch.environment)?);

    if let Some(limit) = scenario.launch.output_byte_budget {
        builder = builder.output_byte_budget(limit);
    }

    if let Some(directory) = &scenario.launch.working_directory {
        builder = builder.working_directory(directory);
    }

    let mut session = builder
        .start()
        .map_err(|error| return operation("launch", error))?;

    let mut steps: Vec<ScenarioStepResult> = Vec::new();

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
                let final_snapshot = send_request(
                    session.path(),
                    &ControlRequest::new(ControlCommand::Snapshot),
                )
                .ok();
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
        session
            .stop(Some(Duration::from_millis(scenario.launch.timeout_ms)))
            .map_err(|error| return operation("stop", error))?;
    }

    if let Err(error) = validate_replay_closure(session.path(), &steps) {
        let failure_artifacts = failure_root.join(session.session_id().to_string());

        let report = ScenarioReport {
            version: SCENARIO_VERSION,
            name: scenario.name.clone(),
            status: ScenarioStatus::Failed,
            session_id: session.session_id(),
            canonical_session: session.path().to_path_buf(),
            steps,
            failure: Some(ScenarioFailure {
                step_index: scenario.steps.len(),
                step_kind: "replay_closure".to_owned(),
                request_ids: Vec::new(),
                message: error.to_string(),
            }),
            failure_artifacts: Some(failure_artifacts.clone()),
        };

        publish_failure(&failure_artifacts, scenario, &report, None)?;

        return Ok(report);
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
) -> StdResult<ScenarioStepResult, ScenarioFailure> {
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
                .screen()
                .wait(
                    predicate,
                    Duration::from_millis(wait.stable_for_ms),
                    Duration::from_millis(wait.timeout_ms.unwrap_or(scenario.launch.timeout_ms)),
                )
                .map_err(|error| return step_failure(index, step, Vec::new(), error.to_string()))?;

            let wait_satisfied = matched.matched
                && if wait.stable_for_ms == 0 {
                    matched.quality == ScreenWaitQuality::MatchedUnsettled
                } else {
                    matched.quality == ScreenWaitQuality::Matched
                };

            if !wait_satisfied {
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
            for input in inputs {
                let command = match input {
                    ScenarioInput::Text(input) => ControlCommand::Input {
                        data: ByteString::new(input.text.as_bytes().to_vec()),
                        input_kind: InputKind::Text,
                        caller_intent: None,
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

            None
        }

        ScenarioStep::Mark(label) => {
            let request = ControlRequest::new(ControlCommand::Mark {
                label: label.clone(),
            });
            request_ids.push(request.request_id);
            session.send(&request).map_err(|error| {
                return step_failure(index, step, request_ids.clone(), error.to_string());
            })?;
            None
        }

        ScenarioStep::MatchScreenSnapshot(snapshot) => {
            let capture = session
                .screen()
                .capture_stable(StableCaptureOptions {
                    stable_for: Duration::from_millis(snapshot.stable_for_ms),
                    timeout: Duration::from_millis(
                        snapshot.timeout_ms.unwrap_or(scenario.launch.timeout_ms),
                    ),
                    projection: Default::default(),
                    allow_incomplete: false,
                })
                .map_err(|error| return step_failure(index, step, Vec::new(), error.to_string()))?;

            let state_hash = capture.frame.state_hash.clone();
            let baseline = rich_frame_snapshot(&capture.frame);
            let path = snapshot_path(failure_root, &snapshot.name)
                .map_err(|error| return step_failure(index, step, Vec::new(), error.to_string()))?;

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
                let bytes = fs_err::read(&path).map_err(|error| {
                    return step_failure(
                        index,
                        step,
                        Vec::new(),
                        format!("read snapshot {}: {error}", path.display()),
                    );
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
            assert_timeline(session.path(), assertion)
                .map_err(|error| return step_failure(index, step, Vec::new(), error.to_string()))?;
            None
        }

        ScenarioStep::Stop {} => {
            let request = ControlRequest::new(ControlCommand::Stop);
            request_ids.push(request.request_id);

            let stopped_already = session.path().join("completion.json").is_file();

            if !stopped_already {
                match session.send(&request) {
                    Ok(_) => {}
                    Err(SdkError::ControlUnavailable { lifecycle, .. })
                        if lifecycle == "completed"
                            || session.path().join("completion.json").is_file() => {}
                    Err(error) => {
                        return Err(step_failure(
                            index,
                            step,
                            request_ids.clone(),
                            error.to_string(),
                        ));
                    }
                }
            }

            session
                .complete(Duration::from_millis(scenario.launch.timeout_ms))
                .map_err(|error| {
                    return step_failure(index, step, request_ids.clone(), error.to_string());
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
) -> ScenarioFailure {
    return ScenarioFailure {
        step_index: index,
        step_kind: step.kind().to_owned(),
        request_ids,
        message: message.into(),
    };
}

fn rich_frame_snapshot(frame: &ScreenFrame) -> ScenarioFrameSnapshot {
    let viewport_text = match &frame.data {
        ScreenFrameData::Text { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameData::Spans { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameData::Cells { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>(),
        ScreenFrameData::None => Vec::new(),
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
        let mut cursor = EventLogCursor::open(snapshot.path().join("events.jsonl"))
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
