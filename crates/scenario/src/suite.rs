use crate::{
    document::{expand_matrix, read},
    error::{invalid, operation},
    execution::{error_report, run_validated},
    Result,
};
use std::{
    collections::VecDeque,
    fmt::Write as _,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Instant,
};
use termtrace_evidence::artifact::write_secure;
use termtrace_model::scenario::{
    Scenario, ScenarioAttemptReport, ScenarioCaseReport, ScenarioCaseStatus,
    ScenarioInfrastructureError, ScenarioReport, ScenarioStatus, ScenarioSuiteReport,
    ScenarioSuiteStatus, SCENARIO_SUITE_REPORT_VERSION,
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
    /// Make recovered expectation failures fail CI (default false).
    pub fail_on_flaky: bool,
}

/// Read, expand, shard, and execute scenario documents with bounded concurrency.
///
/// # Errors
///
/// Returns an error when the suite options are invalid, a scenario document cannot be read or its
/// matrix expanded, or selection is empty. Executed infrastructure failures remain in the report.
pub fn run_suite(
    scenario_paths: impl IntoIterator<Item = PathBuf>,
    options: &ScenarioSuiteOptions,
) -> Result<ScenarioSuiteReport> {
    let started = Instant::now();
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
    // Keep identities independently of worker delivery, so channel loss cannot erase a case.
    let identities: Vec<_> = tasks
        .iter()
        .map(|(p, s)| return (p.clone(), s.name.clone()))
        .collect();

    let queue = Arc::new(Mutex::new(VecDeque::from_iter(
        tasks.into_iter().enumerate(),
    )));

    let (sender, receiver) = mpsc::channel();

    let mut cases: Vec<Option<ScenarioCaseReport>> = vec![None; task_count];
    thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..options.jobs.min(task_count) {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            workers.push(scope.spawn(move || loop {
                let task = queue
                    .lock()
                    .unwrap_or_else(|p| return p.into_inner())
                    .pop_front();
                let Some((index, (path, scenario))) = task else {
                    break;
                };

                let started = Instant::now();
                let result = catch_unwind(AssertUnwindSafe(|| {
                    return run_case(path.clone(), &scenario, options);
                }));

                let case = match result {
                    Ok(case) => case,
                    Err(_) => infrastructure_case(
                        path,
                        &scenario.name,
                        "worker_panic",
                        elapsed_ms(started),
                    ),
                };
                if sender.send((index, case)).is_err() {
                    break;
                }
            }));
        }

        drop(sender);
        // Drain all deliveries before joining every worker. Never return early on a case error.
        for (index, case) in receiver {
            if let Some(slot) = cases.get_mut(index) {
                *slot = Some(case);
            }
        }

        for worker in workers {
            let _joined = worker.join();
        }
    });

    let mut cases: Vec<_> = cases
        .into_iter()
        .zip(identities)
        .map(|(case, (path, name))| {
            return case.unwrap_or_else(|| {
                return infrastructure_case(path, &name, "worker_result_lost", 0);
            });
        })
        .collect();
    cases.sort_by(|a, b| return a.scenario.cmp(&b.scenario));
    let mut report = ScenarioSuiteReport {
        version: SCENARIO_SUITE_REPORT_VERSION,
        status: ScenarioSuiteStatus::Failed,
        duration_ms: elapsed_ms(started),
        fail_on_flaky: options.fail_on_flaky,
        infrastructure_errors: Vec::new(),
        jobs: options.jobs,
        shard_index: shard.map(|s| return s.0),
        shard_count: shard.map(|s| return s.1),
        cases,
    };
    report.status = report.verdict();

    return Ok(report);
}

fn elapsed_ms(started: Instant) -> u64 {
    return u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
}

fn infrastructure_case(
    path: PathBuf,
    name: &str,
    code: &str,
    duration_ms: u64,
) -> ScenarioCaseReport {
    return ScenarioCaseReport {
        scenario: path,
        status: ScenarioCaseStatus::Failed,
        attempts: vec![ScenarioAttemptReport {
            attempt: 1,
            duration_ms,
            report: error_report(name, "orchestration", code),
        }],
    };
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
    scenario: &Scenario,
    options: &ScenarioSuiteOptions,
) -> ScenarioCaseReport {
    return run_case_with(path, &scenario.name, options.reruns, || {
        return run_validated(
            scenario,
            &options.binary,
            &options.evidence_root,
            &options.failure_root,
            options.update_snapshots,
        )
        .unwrap_or_else(|_| {
            return error_report(&scenario.name, "orchestration", "execution_error");
        });
    });
}

fn run_case_with(
    path: PathBuf,
    name: &str,
    reruns: u8,
    mut execute: impl FnMut() -> ScenarioReport,
) -> ScenarioCaseReport {
    let mut case = ScenarioCaseReport {
        scenario: path,
        status: ScenarioCaseStatus::Failed,
        attempts: Vec::new(),
    };
    for attempt in 1..=u32::from(reruns) + 1 {
        let started = Instant::now();
        let report = catch_unwind(AssertUnwindSafe(&mut execute))
            .unwrap_or_else(|_| return error_report(name, "orchestration", "worker_panic"));
        let retry = report.status == ScenarioStatus::Failed;

        case.attempts.push(ScenarioAttemptReport {
            attempt,
            duration_ms: elapsed_ms(started),
            report,
        });

        if !retry {
            break;
        }
    }

    case.status = case.verdict().unwrap_or(ScenarioCaseStatus::Failed);

    return case;
}

fn xml(input: &str, attribute: bool) -> String {
    let mut out = String::new();
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&apos;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t'
            | '\n'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => out.push(c),
            _ => out.push('\u{fffd}'),
        }
    }

    return out;
}

fn seconds(ms: u64) -> String {
    return format!("{}.{:03}", ms / 1000, ms % 1000);
}

fn property(out: &mut String, name: &str, value: &str) {
    let _written = write!(
        out,
        "<property name=\"{}\" value=\"{}\"/>",
        xml(name, true),
        xml(value, true)
    );
}

/// Render deterministic, fail-closed JUnit with every retained attempt and safe evidence references.
#[must_use]
pub fn scenario_junit(report: &ScenarioSuiteReport) -> String {
    let suite_invalid = report.status != report.verdict()
        || report.version != SCENARIO_SUITE_REPORT_VERSION
        || report.cases.is_empty()
        || report.jobs == 0
        || !matches!(
            (report.shard_index, report.shard_count),
            (None, None) | (Some(_), Some(_))
        )
        || report
            .shard_index
            .zip(report.shard_count)
            .is_some_and(|(i, n)| return n == 0 || i >= n);

    let mut failures = 0;
    let mut errors = 0;
    let mut body = String::new();

    for (index, case) in report.cases.iter().enumerate() {
        let verdict = case.verdict();
        let invalid = verdict.is_err() || verdict.is_ok_and(|v| return v != case.status);
        let infrastructure = invalid
            || case
                .attempts
                .iter()
                .any(|a| return a.report.status == ScenarioStatus::Error)
            || (index == 0 && (suite_invalid || !report.infrastructure_errors.is_empty()));

        let failed = !infrastructure
            && (case.status == ScenarioCaseStatus::Failed
                || (case.status == ScenarioCaseStatus::Flaky && report.fail_on_flaky));

        if infrastructure {
            errors += 1;
        } else if failed {
            failures += 1;
        }

        let duration = case
            .attempts
            .iter()
            .fold(0u64, |sum, a| return sum.saturating_add(a.duration_ms));
        let name = case.attempts.last().map_or_else(
            || return case.scenario.display().to_string(),
            |a| return a.report.name.clone(),
        );
        let _written = write!(
            body,
            "<testcase classname=\"{}\" name=\"{}\" time=\"{}\"><properties>",
            xml(&case.scenario.to_string_lossy(), true),
            xml(&name, true),
            seconds(duration)
        );
        property(
            &mut body,
            "verdict",
            if invalid {
                "invalid"
            } else {
                match case.status {
                    ScenarioCaseStatus::Passed => "passed",
                    ScenarioCaseStatus::Flaky => "flaky",
                    ScenarioCaseStatus::Failed => "failed",
                }
            },
        );
        property(
            &mut body,
            "fail_on_flaky",
            if report.fail_on_flaky { "true" } else { "false" },
        );
        body.push_str("</properties>");

        let mut detail = String::new();
        for a in &case.attempts {
            let _written = writeln!(
                detail,
                "attempt {} duration_ms={} status={}",
                a.attempt,
                a.duration_ms,
                match a.report.status {
                    ScenarioStatus::Passed => "passed",
                    ScenarioStatus::Failed => "failed",
                    ScenarioStatus::Error => "error",
                }
            );

            if let Some(f) = &a.report.failure {
                let _written = writeln!(
                    detail,
                    "step {} ({}) {} request_ids={:?}",
                    f.step_index, f.step_kind, f.message, f.request_ids
                );
            }

            for e in &a.report.infrastructure_errors {
                diagnostic(&mut detail, e);
            }

            for w in &a.report.artifact_warnings {
                let _written = writeln!(detail, "artifact warning: {w}");
            }

            if let Some(p) = &a.report.canonical_session {
                let _written = writeln!(detail, "canonical_session={}", p.display());
            }

            if let Some(p) = &a.report.failure_artifacts {
                let _written = writeln!(detail, "failure_artifacts={}", p.display());
            }
        }

        if index == 0 {
            for e in &report.infrastructure_errors {
                diagnostic(&mut detail, e);
            }
        }

        if invalid || suite_invalid {
            detail.push_str("invalid report invariants\n");
        }

        if infrastructure || failed {
            let tag = if infrastructure { "error" } else { "failure" };
            let message = if infrastructure {
                "scenario infrastructure error"
            } else if case.status == ScenarioCaseStatus::Flaky {
                "recovered flake rejected by policy"
            } else {
                "scenario expectations failed"
            };

            let _written = write!(
                body,
                "<{tag} message=\"{message}\">{}</{tag}>",
                xml(&detail, false)
            );
        }

        let _written = write!(
            body,
            "<system-out>{}</system-out></testcase>",
            xml(&detail, false)
        );
    }

    if report.cases.is_empty() {
        errors += 1;
        body.push_str("<system-err>invalid empty suite</system-err>");
    }

    return format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite name=\"termtrace\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" time=\"{}\">{body}</testsuite>", report.cases.len(), seconds(report.duration_ms));
}

fn diagnostic(detail: &mut String, error: &ScenarioInfrastructureError) {
    let _written = writeln!(
        detail,
        "infrastructure {} {}: {}",
        error.phase, error.code, error.message
    );
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
    use crate::execution::infrastructure;
    use termtrace_model::scenario::{ScenarioFailure, ScenarioReport, SCENARIO_REPORT_VERSION};

    fn attempt(status: ScenarioStatus) -> ScenarioReport {
        let mut r = error_report("example", "launch", "recorder_startup");
        if status != ScenarioStatus::Error {
            r.status = status;
            r.infrastructure_errors.clear();
            r.session_id = Some(uuid::Uuid::nil());
            r.canonical_session = Some(PathBuf::from("/canonical.tt"));

            if status == ScenarioStatus::Failed {
                r.failure = Some(ScenarioFailure {
                    step_index: 1,
                    step_kind: "wait_screen".to_owned(),
                    request_ids: vec![],
                    message: "unmet expectation".to_owned(),
                });
            }
        }

        return r;
    }

    fn case(statuses: &[ScenarioStatus]) -> ScenarioCaseReport {
        let mut c = ScenarioCaseReport {
            scenario: PathBuf::from("example.json"),
            status: ScenarioCaseStatus::Failed,
            attempts: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    return ScenarioAttemptReport {
                        attempt: u32::try_from(i + 1).unwrap(),
                        duration_ms: 120,
                        report: attempt(*s),
                    };
                })
                .collect(),
        };
        c.status = c.verdict().unwrap_or(ScenarioCaseStatus::Failed);

        return c;
    }

    fn suite(cases: Vec<ScenarioCaseReport>, strict: bool) -> ScenarioSuiteReport {
        let mut r = ScenarioSuiteReport {
            version: SCENARIO_SUITE_REPORT_VERSION,
            status: ScenarioSuiteStatus::Failed,
            duration_ms: 1234,
            fail_on_flaky: strict,
            infrastructure_errors: vec![],
            jobs: 2,
            shard_index: None,
            shard_count: None,
            cases,
        };
        r.status = r.verdict();

        return r;
    }

    #[test]
    fn reducer_and_retry_truth_table_both_policies() {
        use ScenarioStatus::{Error as E, Failed as F, Passed as P};
        for (statuses, verdict) in [
            (vec![P], ScenarioCaseStatus::Passed),
            (vec![F, P], ScenarioCaseStatus::Flaky),
            (vec![F, F, P], ScenarioCaseStatus::Flaky),
            (vec![F, F, F], ScenarioCaseStatus::Failed),
            (vec![E], ScenarioCaseStatus::Failed),
            (vec![F, E], ScenarioCaseStatus::Failed),
        ] {
            let expected = case(&statuses);
            assert_eq!(expected.status, verdict);
            for strict in [false, true] {
                let r = suite(vec![case(&[P]), expected.clone()], strict);
                assert_eq!(
                    r.ci_failed(),
                    verdict == ScenarioCaseStatus::Failed
                        || (strict && verdict == ScenarioCaseStatus::Flaky)
                );
                assert_eq!(
                    scenario_junit(&r).contains("<failure message="),
                    verdict == ScenarioCaseStatus::Failed && !statuses.contains(&E)
                        || strict && verdict == ScenarioCaseStatus::Flaky
                );
                assert_eq!(
                    scenario_junit(&r).contains("<error message="),
                    statuses.contains(&E)
                );
            }

            let mut count = 0;
            let actual = run_case_with(PathBuf::from("retry.json"), "retry", 2, || {
                let r = attempt(statuses[count]);
                count += 1;

                return r;
            });
            assert_eq!(count, statuses.len());
            assert_eq!(actual.status, verdict);
        }
    }

    #[test]
    fn failed_dominates_flaky_and_suite_infrastructure_under_both_policies() {
        use ScenarioStatus::{Error as E, Failed as F, Passed as P};
        for strict in [false, true] {
            for other in [case(&[F]), case(&[E])] {
                let report = suite(vec![case(&[F, P]), other], strict);
                assert_eq!(report.status, ScenarioSuiteStatus::Failed);
                assert!(report.ci_failed());
            }

            let mut report = suite(vec![case(&[P]), case(&[F, P])], strict);
            report
                .infrastructure_errors
                .push(infrastructure("report_write", "junit_write"));
            report.status = report.verdict();
            assert_eq!(report.status, ScenarioSuiteStatus::Failed);
            assert!(report.ci_failed());
        }
    }

    #[test]
    fn panic_stops_retries_and_retains_primary_attempt() {
        let mut count = 0;
        let case = run_case_with(PathBuf::from("panic.json"), "panic", 3, || {
            count += 1;
            if count == 1 {
                return attempt(ScenarioStatus::Failed);
            }
            panic!("private panic payload must not enter reports")
        });

        assert_eq!(count, 2);
        assert_eq!(case.attempts.len(), 2);

        let r = suite(vec![case], false);
        assert!(r.ci_failed());
        assert!(!scenario_junit(&r).contains("private panic payload"));
    }

    #[test]
    fn malformed_reports_fail_closed_and_versions_are_not_reinterpreted() {
        use ScenarioStatus::{Failed as F, Passed as P};
        let mut invalids = vec![case(&[])];
        let mut gap = case(&[F, P]);
        gap.attempts[1].attempt = 3;
        invalids.push(gap);

        let mut inconsistent = case(&[P]);
        inconsistent.attempts[0].report.failure = attempt(F).failure;
        invalids.push(inconsistent);

        let mut legacy = case(&[P]);
        legacy.attempts[0].report.version = SCENARIO_REPORT_VERSION - 1;
        invalids.push(legacy);
        invalids.push(case(&[P, P]));

        let mut wrong_verdict = case(&[F, P]);
        wrong_verdict.status = ScenarioCaseStatus::Passed;
        invalids.push(wrong_verdict);
        for c in invalids {
            let r = suite(vec![c], false);
            assert!(r.ci_failed());
            assert!(scenario_junit(&r).contains("<error message="));
        }

        let mut r = suite(vec![case(&[P])], false);
        r.infrastructure_errors
            .push(infrastructure("report_write", "junit_write"));
        r.status = r.verdict();

        assert!(r.ci_failed());
        assert!(scenario_junit(&r).contains("errors=\"1\""));

        let mut value = serde_json::to_value(suite(vec![case(&[P])], false)).unwrap();
        value["version"] = 1.into();

        assert!(serde_json::from_value::<ScenarioSuiteReport>(value).is_err());

        let mut value = serde_json::to_value(attempt(P)).unwrap();
        value["version"] = 2.into();

        assert!(serde_json::from_value::<ScenarioReport>(value).is_err());
    }

    #[test]
    fn junit_all_attempts_durations_identities_and_secure_publication() {
        let r = suite(
            vec![case(&[ScenarioStatus::Failed, ScenarioStatus::Passed])],
            false,
        );

        let xml = scenario_junit(&r);
        assert_eq!(xml, scenario_junit(&r));
        for expected in [
            "time=\"1.234\"",
            "time=\"0.240\"",
            "classname=\"example.json\"",
            "attempt 1 duration_ms=120 status=failed",
            "attempt 2 duration_ms=120 status=passed",
            "verdict\" value=\"flaky",
            "fail_on_flaky\" value=\"false",
        ] {
            assert!(xml.contains(expected), "{expected}");
        }

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("junit.xml");
        write_scenario_junit(&path, &r).unwrap();

        assert_eq!(fs_err::read_to_string(&path).unwrap(), xml);
        assert!(write_scenario_junit(&path, &r).is_err());
        assert!(write_scenario_junit(temp.path(), &r).is_err());
    }

    #[test]
    fn selection_options_are_preflight_errors() {
        let mut options = ScenarioSuiteOptions {
            binary: "/missing".into(),
            evidence_root: "/tmp/evidence".into(),
            failure_root: "/tmp/failures".into(),
            jobs: 1,
            shard_index: None,
            shard_count: None,
            update_snapshots: false,
            reruns: 0,
            fail_on_flaky: false,
        };

        assert_eq!(validate_suite_options(&options).unwrap(), None);

        options.jobs = 0;

        assert!(validate_suite_options(&options).is_err());

        options.jobs = 1;
        options.shard_index = Some(0);

        assert!(validate_suite_options(&options).is_err());

        options.shard_count = Some(0);

        assert!(validate_suite_options(&options).is_err());

        options.shard_count = Some(2);

        assert_eq!(validate_suite_options(&options).unwrap(), Some((0, 2)));

        options.shard_index = Some(2);

        assert!(validate_suite_options(&options).is_err());
    }
}
