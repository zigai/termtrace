//! Scenario suites: running documents, JUnit publication, and the shared CI verdict.

use crate::host::{Arg, Model};
use crate::{blocking_failure, to_napi_err};
use napi_derive::napi;
use termtrace_model::scenario::ScenarioSuiteReport;
use termtrace_model::value::ExactPath;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::scenario::{self, run_suite, ScenarioSuiteOptions};

fn scenario_err(error: scenario::Error) -> napi::Error {
    return to_napi_err(termtrace_sdk::Error::InvalidOperation {
        reason: error.to_string(),
        source: Some(Box::new(error)),
    });
}

/// Selection, concurrency, snapshot, and rerun settings of one scenario suite.
#[napi(object, object_to_js = false, js_name = "ScenarioSuiteOptions")]
pub struct JsScenarioSuiteOptions {
    /// Parent of failure artifacts; snapshot baselines live in its sibling `snapshots`.
    pub failure_root: Arg<ExactPath>,
    /// Parent of each case's fresh session; the client's evidence root when omitted.
    pub evidence_root: Option<Arg<ExactPath>>,
    /// Maximum cases run at once; 1 when omitted.
    pub jobs: Option<u32>,
    /// Zero-based shard index; requires `shardCount`.
    pub shard_index: Option<u32>,
    /// Total shard count; requires `shardIndex`.
    pub shard_count: Option<u32>,
    /// Replace snapshot baselines instead of comparing them.
    pub update_snapshots: Option<bool>,
    /// Fresh-session reruns of a failed case; 0 when omitted.
    pub reruns: Option<u8>,
    /// Make a flaky case fail CI.
    pub fail_on_flaky: Option<bool>,
}

pub(crate) async fn run(
    terminal: &Terminal,
    scenarios: Vec<Arg<ExactPath>>,
    options: JsScenarioSuiteOptions,
) -> napi::Result<Model<ScenarioSuiteReport>> {
    let options = ScenarioSuiteOptions {
        binary: terminal.binary().to_path_buf(),
        evidence_root: options.evidence_root.map_or_else(
            || return terminal.evidence_root().to_path_buf(),
            |root| return root.0 .0,
        ),
        failure_root: options.failure_root.0 .0,
        jobs: options.jobs.map_or(1, |jobs| return jobs as usize),
        shard_index: options.shard_index.map(|index| return index as usize),
        shard_count: options.shard_count.map(|count| return count as usize),
        update_snapshots: options.update_snapshots.unwrap_or(false),
        reruns: options.reruns.unwrap_or(0),
        fail_on_flaky: options.fail_on_flaky.unwrap_or(false),
    };

    let paths = scenarios.into_iter().map(|path| return path.0 .0);

    return tokio::task::spawn_blocking(move || return run_suite(paths, &options))
        .await
        .map_err(blocking_failure)?
        .map(Model)
        .map_err(scenario_err);
}

/// Write `report` as JUnit XML to the new file `path`, one `testcase` per case with every
/// attempt in its output. A failed case, or a flaky one under `fail_on_flaky`, is a `failure`;
/// an infrastructure error is an `error`.
///
/// # Errors
///
/// Rejects when the file cannot be securely written.
#[napi]
pub async fn write_scenario_junit(
    path: Arg<ExactPath>,
    report: Arg<ScenarioSuiteReport>,
) -> napi::Result<()> {
    return tokio::task::spawn_blocking(move || {
        return scenario::write_scenario_junit(&path.0 .0, &report.0);
    })
    .await
    .map_err(blocking_failure)?
    .map_err(scenario_err);
}

/// Whether CI must fail for `report`: any failed case, an infrastructure error, a flaky case
/// under `fail_on_flaky`, or a `status` that does not match the cases.
#[napi]
pub fn scenario_ci_failed(report: Arg<ScenarioSuiteReport>) -> bool {
    return report.0.ci_failed();
}
