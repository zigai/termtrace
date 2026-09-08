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

use crate::{
    document::{expand_matrix, read},
    error::{invalid, operation},
    execution::run_validated,
    Error, Result,
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
        w.write_attribute("name", &name);

        if let Some(failure) =
            final_attempt.and_then(|attempt| return attempt.report.failure.as_ref())
        {
            w.start_element("failure");
            w.write_attribute("message", &failure.message);
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
pub fn write_scenario_junit(path: impl AsRef<Path>, report: &ScenarioSuiteReport) -> Result<()> {
    write_secure(path.as_ref(), scenario_junit(report).as_bytes())
        .map_err(|error| return operation("write JUnit report", error))?;

    return Ok(());
}
