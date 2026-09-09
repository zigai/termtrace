use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::output::write_json_line;
use args::TestArgs;
use miette::{IntoDiagnostic as _, Result};
use std::{env, process::ExitCode};
use termtrace_model::scenario::ScenarioSuiteStatus;
use termtrace_scenario::{run_suite, write_scenario_junit, ScenarioSuiteOptions};

pub(super) fn run_test(response: &mut ResponseWriter, arguments: TestArgs) -> Result<ExitCode> {
    let binary = env::current_exe().into_diagnostic()?;
    let options = ScenarioSuiteOptions {
        binary,
        evidence_root: arguments.artifacts.join("sessions"),
        failure_root: arguments.artifacts.join("failures"),
        jobs: arguments.jobs,
        shard_index: arguments.shard_index,
        shard_count: arguments.shard_count,
        update_snapshots: arguments.update_snapshots,
        reruns: arguments.reruns,
    };

    let report = run_suite(arguments.scenario, &options).into_diagnostic()?;

    if let Some(path) = arguments.junit {
        write_scenario_junit(path, &report).into_diagnostic()?;
    }

    let status = report.status;
    write_json_line(response, &report)?;

    return Ok(if status == ScenarioSuiteStatus::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    });
}
