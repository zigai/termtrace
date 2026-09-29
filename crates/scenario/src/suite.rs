use crate::{
    document::{expand_matrix, read},
    error::{invalid, operation},
    execution::run_validated,
    Error, Result,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    result::Result as StdResult,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use termtrace_evidence::artifact::write_secure;
use termtrace_model::scenario::{
    Scenario, ScenarioAttemptReport, ScenarioCaseReport, ScenarioStatus, ScenarioSuiteReport,
    ScenarioSuiteStatus,
};

/// Execution and selection settings for a deterministic scenario suite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioSuiteOptions {
    /// Recorder/CLI executable used by the SDK launcher.
    pub binary: PathBuf,
    /// Parent directory for canonical sessions.
    pub evidence_root: PathBuf,
    /// Parent directory for atomic failure artifacts and snapshot baselines.
    pub failure_root: PathBuf,
    /// Maximum concurrent fresh sessions.
    pub jobs: usize,
    /// Optional zero-based shard index.
    pub shard_index: Option<usize>,
    /// Optional total shard count.
    pub shard_count: Option<usize>,
    /// Replace normalized screen baselines instead of comparing them.
    pub update_snapshots: bool,
    /// Number of fresh-session reruns after the initial attempt.
    pub reruns: u8,
}

/// Read, expand, shard, and execute scenario documents with bounded concurrency.
///
/// # Errors
///
/// Returns an error when the suite options are invalid, a scenario document cannot be read or its
/// matrix expanded, or a scenario cannot be executed.
pub fn run_suite(
    scenario_paths: impl IntoIterator<Item = PathBuf>,
    options: &ScenarioSuiteOptions,
) -> Result<ScenarioSuiteReport> {
    let shard = validate_suite_options(options)?;
    let mut paths: Vec<PathBuf> = scenario_paths.into_iter().collect();
    paths.sort();
    paths.dedup();

    let mut expanded: Vec<(PathBuf, Scenario)> = Vec::new();
    for path in paths {
        let scenario = read(&path)?;

        for (label, scenario) in expand_matrix(scenario)? {
            let identity = if label.is_empty() {
                path.clone()
            } else {
                PathBuf::from(format!("{}#{label}", path.display()))
            };
            expanded.push((identity, scenario));
        }
    }

    let tasks: Vec<(PathBuf, Scenario)> = expanded
        .into_iter()
        .enumerate()
        .filter(|(index, _)| {
            return shard.is_none_or(|(shard_index, shard_count)| {
                return index % shard_count == shard_index;
            });
        })
        .map(|(_, task)| return task)
        .collect();

    if tasks.is_empty() {
        return Err(invalid("scenario selection produced an empty shard"));
    }

    let task_count = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (sender, receiver) = mpsc::channel::<StdResult<ScenarioCaseReport, String>>();
    let mut cases: Vec<Option<ScenarioCaseReport>> = (0..task_count).map(|_| return None).collect();

    thread::scope(|scope| {
        for _worker in 0..options.jobs.min(task_count) {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let options = options.clone();

            scope.spawn(move || loop {
                let task = {
                    let mut guard = match queue.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    guard.pop_front()
                };
                let Some((path, scenario)) = task else {
                    return;
                };
                let result =
                    run_case(path, scenario, &options).map_err(|error| return error.to_string());
                if sender.send(result).is_err() {
                    return;
                }
            });
        }

        drop(sender);

        for slot in &mut cases {
            let case = receiver
                .recv()
                .map_err(|error| return operation("receive suite result", error))?
                .map_err(|message| {
                    return Error::Operation {
                        operation: "execute suite case",
                        message,
                    };
                })?;
            *slot = Some(case);
        }

        return Ok::<(), Error>(());
    })?;

    let mut cases: Vec<ScenarioCaseReport> = cases.into_iter().flatten().collect();
    cases.sort_by(|left, right| return left.scenario.cmp(&right.scenario));
    let status = if cases.iter().all(|case| {
        return case
            .attempts
            .last()
            .is_some_and(|attempt| return attempt.report.status == ScenarioStatus::Passed);
    }) {
        ScenarioSuiteStatus::Passed
    } else {
        ScenarioSuiteStatus::Failed
    };

    return Ok(ScenarioSuiteReport {
        version: 1,
        status,
        jobs: options.jobs,
        shard_index: shard.map(|value| return value.0),
        shard_count: shard.map(|value| return value.1),
        cases,
    });
}

fn validate_suite_options(options: &ScenarioSuiteOptions) -> Result<Option<(usize, usize)>> {
    if options.jobs == 0 {
        return Err(invalid("scenario jobs must be greater than zero"));
    }

    match (options.shard_index, options.shard_count) {
        (Some(index), Some(count)) if count > 0 && index < count => {
            return Ok(Some((index, count)));
        }
        (Some(index), Some(count)) => {
            return Err(invalid(format!(
                "scenario shard index {index} must be less than non-zero shard count {count}"
            )));
        }
        (None, None) => return Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            return Err(invalid("scenario sharding requires index and count"));
        }
    }
}

fn run_case(
    path: PathBuf,
    scenario: Scenario,
    options: &ScenarioSuiteOptions,
) -> Result<ScenarioCaseReport> {
    let mut attempts: Vec<ScenarioAttemptReport> = Vec::new();
    for attempt in 1..=u32::from(options.reruns).saturating_add(1) {
        let report = run_validated(
            &scenario,
            &options.binary,
            &options.evidence_root,
            &options.failure_root,
            options.update_snapshots,
        )?;

        let passed = report.status == ScenarioStatus::Passed;

        attempts.push(ScenarioAttemptReport { attempt, report });

        if passed {
            break;
        }
    }

    return Ok(ScenarioCaseReport {
        scenario: path,
        attempts,
    });
}

/// Escape special XML characters for safe attribute inclusion.
fn escape_xml_attribute(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }

    return escaped;
}

/// Render deterministic JUnit XML for a suite report.
#[must_use]
pub fn scenario_junit(report: &ScenarioSuiteReport) -> String {
    let failure_count = report
        .cases
        .iter()
        .filter(|case| {
            return case
                .attempts
                .last()
                .is_none_or(|attempt| return attempt.report.status == ScenarioStatus::Failed);
        })
        .count();

    let mut w = xmlwriter::XmlWriter::new(xmlwriter::Options::default());
    w.start_element("testsuite");
    w.write_attribute("name", "termtrace");
    w.write_attribute("tests", &report.cases.len());
    w.write_attribute("failures", &failure_count);

    for case in &report.cases {
        let final_attempt = case.attempts.last();
        let name = final_attempt.map_or_else(
            || return case.scenario.display().to_string(),
            |attempt| return attempt.report.name.clone(),
        );

        w.start_element("testcase");
        w.write_attribute("name", &escape_xml_attribute(&name));

        if let Some(failure) =
            final_attempt.and_then(|attempt| return attempt.report.failure.as_ref())
        {
            w.start_element("failure");
            w.write_attribute("message", &escape_xml_attribute(&failure.message));
            w.write_text(&format!(
                "step {} ({})",
                failure.step_index, failure.step_kind
            ));
            w.end_element();
        }

        w.end_element();
    }

    w.end_element();

    return w.end_document();
}

/// Securely publish deterministic JUnit XML for a suite report.
///
/// # Errors
///
/// Returns an error when the report cannot be securely written to `path`.
pub fn write_scenario_junit(path: impl AsRef<Path>, report: &ScenarioSuiteReport) -> Result<()> {
    write_secure(path.as_ref(), scenario_junit(report).as_bytes())
        .map_err(|error| return operation("write JUnit report", error))?;

    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use termtrace_model::scenario::{
        ScenarioAttemptReport, ScenarioCaseReport, ScenarioFailure, ScenarioReport, ScenarioStatus,
        ScenarioStepResult, ScenarioSuiteReport, ScenarioSuiteStatus, SCENARIO_VERSION,
    };
    use uuid::Uuid;

    fn sample_passing_report(name: &str) -> ScenarioReport {
        return ScenarioReport {
            version: SCENARIO_VERSION,
            name: name.to_owned(),
            status: ScenarioStatus::Passed,
            session_id: Uuid::now_v7(),
            canonical_session: PathBuf::from("/evidence/pass.tt"),
            steps: vec![ScenarioStepResult {
                index: 0,
                kind: "wait_screen".to_owned(),
                request_ids: Vec::new(),
                state_hash: Some("hash1".to_owned()),
            }],
            failure: None,
            failure_artifacts: None,
        };
    }

    fn sample_failing_report(
        name: &str,
        step_index: usize,
        step_kind: &str,
        failure_message: &str,
    ) -> ScenarioReport {
        return ScenarioReport {
            version: SCENARIO_VERSION,
            name: name.to_owned(),
            status: ScenarioStatus::Failed,
            session_id: Uuid::now_v7(),
            canonical_session: PathBuf::from("/evidence/fail.tt"),
            steps: vec![ScenarioStepResult {
                index: 0,
                kind: "wait_screen".to_owned(),
                request_ids: Vec::new(),
                state_hash: Some("hash1".to_owned()),
            }],
            failure: Some(ScenarioFailure {
                step_index,
                step_kind: step_kind.to_owned(),
                request_ids: vec![Uuid::now_v7()],
                message: failure_message.to_owned(),
            }),
            failure_artifacts: Some(PathBuf::from("/evidence/failures/1")),
        };
    }

    fn make_suite_report(cases: Vec<ScenarioCaseReport>) -> ScenarioSuiteReport {
        let status = if cases.iter().all(|case| {
            return case
                .attempts
                .last()
                .is_some_and(|attempt| return attempt.report.status == ScenarioStatus::Passed);
        }) {
            ScenarioSuiteStatus::Passed
        } else {
            ScenarioSuiteStatus::Failed
        };

        return ScenarioSuiteReport {
            version: 1,
            status,
            jobs: 2,
            shard_index: None,
            shard_count: None,
            cases,
        };
    }

    #[test]
    fn junit_formatting_with_passing_scenario_report() {
        let report = make_suite_report(vec![
            ScenarioCaseReport {
                scenario: PathBuf::from("scenarios/login.json"),
                attempts: vec![ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_passing_report("login-workflow"),
                }],
            },
            ScenarioCaseReport {
                scenario: PathBuf::from("scenarios/logout.json"),
                attempts: vec![ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_passing_report("logout-workflow"),
                }],
            },
        ]);

        let xml = scenario_junit(&report);
        assert!(xml.contains("<testsuite name=\"termtrace\" tests=\"2\" failures=\"0\">"));
        assert!(xml.contains("<testcase name=\"login-workflow\"/>"));
        assert!(xml.contains("<testcase name=\"logout-workflow\"/>"));
        assert!(!xml.contains("<failure"));
        assert!(xml.trim_end().ends_with("</testsuite>"));
    }

    #[test]
    fn junit_formatting_with_failing_scenario_report() {
        let report = make_suite_report(vec![
            ScenarioCaseReport {
                scenario: PathBuf::from("scenarios/timeout.json"),
                attempts: vec![ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_failing_report(
                        "timeout-scenario",
                        2,
                        "wait_screen",
                        "step timed out after 5000ms duration (deadline exceeded)",
                    ),
                }],
            },
            ScenarioCaseReport {
                scenario: PathBuf::from("scenarios/passing.json"),
                attempts: vec![ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_passing_report("passing-scenario"),
                }],
            },
        ]);

        let xml = scenario_junit(&report);
        assert!(xml.contains("<testsuite name=\"termtrace\" tests=\"2\" failures=\"1\">"));
        assert!(xml.contains("<testcase name=\"timeout-scenario\">"));
        assert!(xml.contains(
            "<failure message=\"step timed out after 5000ms duration (deadline exceeded)\">"
        ));
        assert!(xml.contains("step 2 (wait_screen)"));
        assert!(xml.contains("</failure>"));
        assert!(xml.contains("<testcase name=\"passing-scenario\"/>"));
    }

    #[test]
    fn junit_formatting_with_retried_scenarios() {
        // Flaky test: attempt 1 failed with duration detail, attempt 2 passed
        let recovered_report = make_suite_report(vec![ScenarioCaseReport {
            scenario: PathBuf::from("scenarios/flaky.json"),
            attempts: vec![
                ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_failing_report(
                        "flaky-scenario",
                        1,
                        "wait_screen",
                        "transient failure after 120ms duration",
                    ),
                },
                ScenarioAttemptReport {
                    attempt: 2,
                    report: sample_passing_report("flaky-scenario"),
                },
            ],
        }]);

        let recovered_xml = scenario_junit(&recovered_report);
        assert!(recovered_xml.contains("tests=\"1\" failures=\"0\""));
        assert!(recovered_xml.contains("<testcase name=\"flaky-scenario\"/>"));
        assert!(!recovered_xml.contains("<failure"));

        // Exhausted retries: attempt 1 failed, attempt 2 also failed
        let exhausted_report = make_suite_report(vec![ScenarioCaseReport {
            scenario: PathBuf::from("scenarios/persistent.json"),
            attempts: vec![
                ScenarioAttemptReport {
                    attempt: 1,
                    report: sample_failing_report(
                        "persistent-failure",
                        1,
                        "wait_screen",
                        "initial failure after 50ms duration",
                    ),
                },
                ScenarioAttemptReport {
                    attempt: 2,
                    report: sample_failing_report(
                        "persistent-failure",
                        1,
                        "wait_screen",
                        "retry failure after 150ms duration",
                    ),
                },
            ],
        }]);

        let exhausted_xml = scenario_junit(&exhausted_report);
        assert!(exhausted_xml.contains("tests=\"1\" failures=\"1\""));
        assert!(exhausted_xml.contains("<failure message=\"retry failure after 150ms duration\">"));
        assert!(exhausted_xml.contains("step 1 (wait_screen)"));
        assert!(exhausted_xml.contains("</failure>"));
    }

    #[test]
    fn junit_empty_reports_handling() {
        // Entire suite empty
        let empty_suite = make_suite_report(Vec::new());
        let xml = scenario_junit(&empty_suite);
        assert_eq!(
            xml.trim(),
            "<testsuite name=\"termtrace\" tests=\"0\" failures=\"0\"/>"
        );

        // Case with empty attempts falls back to scenario path and counts as failure
        let empty_attempts_report = make_suite_report(vec![ScenarioCaseReport {
            scenario: PathBuf::from("scenarios/unattempted.json"),
            attempts: Vec::new(),
        }]);

        let xml_empty_attempts = scenario_junit(&empty_attempts_report);
        assert!(xml_empty_attempts.contains("tests=\"1\" failures=\"1\""));
        assert!(xml_empty_attempts.contains("<testcase name=\"scenarios/unattempted.json\"/>"));
    }

    #[test]
    fn junit_xml_escaping_of_special_characters() {
        let special_name = "scenario <name> & 'chars' \"quote\"";
        let special_error =
            "predicate <condition> failed & timed out after 3000ms duration: 'foo' != \"bar\"";

        let report = make_suite_report(vec![ScenarioCaseReport {
            scenario: PathBuf::from("scenarios/special.json"),
            attempts: vec![ScenarioAttemptReport {
                attempt: 1,
                report: sample_failing_report(special_name, 0, "wait_screen", special_error),
            }],
        }]);

        let xml = scenario_junit(&report);
        assert!(xml
            .contains("name=\"scenario &lt;name&gt; &amp; &apos;chars&apos; &quot;quote&quot;\""));
        assert!(xml.contains("message=\"predicate &lt;condition&gt; failed &amp; timed out after 3000ms duration: &apos;foo&apos; != &quot;bar&quot;\""));
        assert!(!xml.contains("<name>"));
        assert!(!xml.contains("<condition>"));
    }

    #[test]
    fn validate_suite_options_combinations() {
        let base = ScenarioSuiteOptions {
            binary: PathBuf::from("/bin/termtrace"),
            evidence_root: PathBuf::from("/tmp/evidence"),
            failure_root: PathBuf::from("/tmp/failures"),
            jobs: 1,
            shard_index: None,
            shard_count: None,
            update_snapshots: false,
            reruns: 0,
        };

        // Zero jobs is rejected
        let mut zero_jobs = base.clone();
        zero_jobs.jobs = 0;

        assert!(validate_suite_options(&zero_jobs).is_err());

        // No sharding is accepted
        assert_eq!(validate_suite_options(&base).expect("valid options"), None);

        // Valid sharding
        let mut sharded = base.clone();
        sharded.shard_index = Some(0);
        sharded.shard_count = Some(2);

        assert_eq!(
            validate_suite_options(&sharded).expect("valid sharding"),
            Some((0, 2))
        );

        let mut last_shard = base.clone();
        last_shard.shard_index = Some(1);
        last_shard.shard_count = Some(2);

        assert_eq!(
            validate_suite_options(&last_shard).expect("valid sharding"),
            Some((1, 2))
        );

        // Shard index >= shard count is rejected
        let mut out_of_bounds = base.clone();
        out_of_bounds.shard_index = Some(2);
        out_of_bounds.shard_count = Some(2);

        assert!(validate_suite_options(&out_of_bounds).is_err());

        // Shard count 0 is rejected
        let mut zero_count = base.clone();
        zero_count.shard_index = Some(0);
        zero_count.shard_count = Some(0);

        assert!(validate_suite_options(&zero_count).is_err());

        // Incomplete sharding options are rejected
        let mut missing_count = base.clone();
        missing_count.shard_index = Some(0);
        missing_count.shard_count = None;

        assert!(validate_suite_options(&missing_count).is_err());

        let mut missing_index = base;
        missing_index.shard_index = None;
        missing_index.shard_count = Some(2);

        assert!(validate_suite_options(&missing_index).is_err());
    }

    #[test]
    fn write_scenario_junit_persists_expected_content() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let junit_path = temporary.path().join("junit.xml");

        let report = make_suite_report(vec![ScenarioCaseReport {
            scenario: PathBuf::from("scenarios/test.json"),
            attempts: vec![ScenarioAttemptReport {
                attempt: 1,
                report: sample_passing_report("persisted-test"),
            }],
        }]);

        write_scenario_junit(&junit_path, &report).expect("write junit");
        let written = fs_err::read_to_string(&junit_path).expect("read junit");
        assert_eq!(written, scenario_junit(&report));
    }
}
