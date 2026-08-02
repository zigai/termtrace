use super::*;

pub(super) fn run_test(arguments: TestArgs) -> Result<ExitCode> {
    let shard = match (arguments.shard_index, arguments.shard_count) {
        (Some(index), Some(count)) if index < count => Some((index, count)),
        (Some(index), Some(count)) => {
            return Err(miette::miette!(
                "scenario shard index {index} must be less than shard count {count}"
            ));
        }
        (None, None) => None,
        _ => {
            return Err(miette::miette!(
                "scenario sharding requires index and count"
            ));
        }
    };
    let mut paths = arguments.scenario;
    paths.sort();
    paths.dedup();
    let tasks = paths
        .into_iter()
        .map(|path| {
            let scenario = Scenario::read(&path)?;
            return scenario
                .expand_matrix()?
                .into_iter()
                .map(|(label, scenario)| {
                    let identity = if label.is_empty() {
                        path.clone()
                    } else {
                        PathBuf::from(format!("{}#{label}", path.display()))
                    };
                    return Ok((identity, scenario));
                })
                .collect::<Result<Vec<_>>>();
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(index, _task)| {
            return shard.is_none_or(|(shard_index, shard_count)| {
                return index % shard_count == shard_index;
            });
        })
        .map(|(_index, task)| return task)
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Err(miette::miette!(
            "scenario selection produced an empty shard"
        ));
    }
    let binary = env::current_exe().into_diagnostic()?;
    let task_count = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (sender, receiver) = mpsc::channel();
    let mut cases = vec![None; task_count];
    thread::scope(|scope| -> Result<()> {
        for _worker in 0..arguments.jobs.min(task_count) {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let binary = binary.clone();
            let artifacts = arguments.artifacts.clone();
            scope.spawn(move || {
                loop {
                    let task = {
                        let mut queue = match queue.lock() {
                            Ok(queue) => queue,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        queue.pop_front()
                    };
                    let Some((path, scenario)) = task else {
                        return;
                    };
                    let result = run_scenario_case(
                        path,
                        scenario,
                        &binary,
                        &artifacts,
                        arguments.update_snapshots,
                        arguments.reruns,
                    )
                    .map_err(|error| return error.to_string());
                    if sender.send(result).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        for slot in &mut cases {
            let case = receiver
                .recv()
                .into_diagnostic()?
                .map_err(|message| return miette::miette!(message))?;
            *slot = Some(case);
        }
        return Ok(());
    })?;
    let mut cases = cases.into_iter().flatten().collect::<Vec<_>>();
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
    let report = ScenarioSuiteReport {
        version: 1,
        status,
        jobs: arguments.jobs,
        shard_index: shard.map(|value| return value.0),
        shard_count: shard.map(|value| return value.1),
        cases,
    };
    if let Some(path) = &arguments.junit {
        write_scenario_junit(path, &report)?;
    }
    write_json_line(&report)?;
    return Ok(if status == ScenarioSuiteStatus::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    });
}

fn run_scenario_case(
    path: PathBuf,
    scenario: Scenario,
    binary: &Path,
    artifacts: &Path,
    update_snapshots: bool,
    reruns: u8,
) -> Result<ScenarioCaseReport> {
    let mut attempts = Vec::new();
    for attempt in 1..=u32::from(reruns).saturating_add(1) {
        let report = run_scenario(
            &scenario,
            binary,
            artifacts.join("sessions"),
            artifacts.join("failures"),
            update_snapshots,
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

fn write_scenario_junit(path: &Path, report: &ScenarioSuiteReport) -> Result<()> {
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
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"termtrace\" tests=\"{}\" failures=\"{failure_count}\">",
        report.cases.len()
    );
    for case in &report.cases {
        let final_attempt = case.attempts.last();
        let name = final_attempt.map_or_else(
            || return case.scenario.display().to_string(),
            |attempt| return attempt.report.name.clone(),
        );
        write!(xml, "\n  <testcase name=\"{}\">", xml_escape(&name)).into_diagnostic()?;
        if let Some(failure) =
            final_attempt.and_then(|attempt| return attempt.report.failure.as_ref())
        {
            write!(
                xml,
                "\n    <failure message=\"{}\">step {} ({})</failure>\n  ",
                xml_escape(&failure.message),
                failure.step_index,
                xml_escape(&failure.step_kind)
            )
            .into_diagnostic()?;
        }
        write!(xml, "</testcase>").into_diagnostic()?;
    }
    writeln!(xml, "\n</testsuite>").into_diagnostic()?;
    write_secure(path, xml.as_bytes())?;
    return Ok(());
}

fn xml_escape(value: &str) -> String {
    return value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
}
