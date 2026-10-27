use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::output::response_output;
use args::TestArgs;
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    io::{stderr, Write as _},
    process::ExitCode,
};
use termtrace_model::scenario::ScenarioInfrastructureError;
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
        fail_on_flaky: arguments.fail_on_flaky,
    };
    let mut report = run_suite(arguments.scenario, &options).into_diagnostic()?;

    if let Some(path) = arguments.junit
        && write_scenario_junit(path, &report).is_err()
    {
        report
            .infrastructure_errors
            .push(ScenarioInfrastructureError {
                phase: "report_write".to_owned(),
                code: "junit_write".to_owned(),
                message: "could not securely publish JUnit report".to_owned(),
            });
        report.status = report.verdict();
        // A failed diagnostic sink must not prevent the authoritative JSON attempt.
        let _diagnostic = writeln!(
            stderr().lock(),
            "scenario report_write failed (junit_write); JSON report remains authoritative"
        );
    }

    let failed = report.ci_failed();
    // Unlike streaming/list commands, losing a CI verdict is a reporting failure,
    // including EPIPE. Use the strict sink rather than the generic early-consumer policy.
    let mut encoded = serde_json::to_vec(&report).into_diagnostic()?;
    encoded.push(b'\n');
    response_output(response)
        .write_all(&encoded)
        .into_diagnostic()?;

    return Ok(if !failed { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}
