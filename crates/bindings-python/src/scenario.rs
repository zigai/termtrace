//! Scenario suite reports: JUnit publication and the shared CI verdict.

use crate::errors::to_py_err;
use crate::host::{Arg, PathArg};
use pyo3::prelude::*;
use pyo3_stub_gen::derive::gen_stub_pyfunction;
use termtrace_model::scenario::ScenarioSuiteReport;
use termtrace_sdk::scenario::{self, write_scenario_junit as write_junit};

pub(crate) fn scenario_err(error: scenario::Error) -> PyErr {
    return to_py_err(termtrace_sdk::Error::InvalidOperation {
        reason: error.to_string(),
        source: Some(Box::new(error)),
    });
}

/// Write `report` as JUnit XML to the new file `path`, one `testcase` per case with every
/// attempt in its output. A failed case, or a flaky one under `fail_on_flaky`, is a `failure`;
/// an infrastructure error is an `error`.
///
/// # Errors
///
/// Raises `TermtraceError` when the file cannot be securely written.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn write_scenario_junit(
    py: Python<'_>,
    path: PathArg,
    report: Arg<ScenarioSuiteReport>,
) -> PyResult<()> {
    return py.detach(move || return write_junit(&path.0, &report.0).map_err(scenario_err));
}

/// Whether CI must fail for `report`: any failed case, an infrastructure error, a flaky case
/// under `fail_on_flaky`, or a `status` that does not match the cases.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn scenario_ci_failed(report: Arg<ScenarioSuiteReport>) -> bool {
    return report.0.ci_failed();
}

/// Register the scenario report functions.
///
/// # Errors
///
/// Fails when a function cannot be added to the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(write_scenario_junit, m)?)?;
    m.add_function(wrap_pyfunction!(scenario_ci_failed, m)?)?;

    return Ok(());
}
