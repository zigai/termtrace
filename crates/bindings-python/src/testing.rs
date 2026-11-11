//! Thin adapters for shared immutable assertions and owned-session finalization.

use crate::{
    errors::{query_err, to_py_err, typed, ExpectationMismatchError},
    host::{to_python, Arg, Model},
    locator::{PyQueryResult, PyScreenMatch},
    recording::PyRecording,
    screen::PyFrame,
};
use pyo3::{exceptions::PyValueError, prelude::*};
use pyo3_stub_gen::derive::gen_stub_pyfunction;
use std::{path::Path, time::Duration};
use termtrace_model::{
    protocol::mutation::MutationEffect,
    terminal::{
        screen::{ScreenRegion, ScreenSnapshotOptions},
        state::CursorState,
    },
};
use termtrace_sdk::locator::StyleFilter;
use termtrace_sdk::{assertions as core, testing as lifecycle};

fn mismatch(error: core::ExpectationMismatchError) -> PyErr {
    let exception = typed(
        ExpectationMismatchError::new_err(error.to_string()),
        "expectation_mismatch",
        "expectation_mismatch",
        MutationEffect::NotApplied,
    );

    return Python::attach(|py| {
        let value = exception.value(py);
        let attributes = to_python(py, &error);

        match attributes.and_then(|attributes| {
            let dict = attributes.cast::<pyo3::types::PyDict>()?;
            for (key, item) in dict.iter() {
                value.setattr(key.cast::<pyo3::types::PyString>()?, item)?;
            }

            return Ok(());
        }) {
            Ok(()) => exception,
            Err(failure) => {
                failure.set_cause(py, Some(exception));
                failure
            }
        }
    });
}

fn assertion(error: core::AssertionError) -> PyErr {
    return match error {
        core::AssertionError::Mismatch(error) => mismatch(error),
        core::AssertionError::Query(error) => query_err(&error),
        core::AssertionError::Snapshot(error) => PyValueError::new_err(error.to_string()),
    };
}

fn terminal_error(error: lifecycle::TerminalAssertionError) -> PyErr {
    return match error {
        lifecycle::TerminalAssertionError::Assertion(error) => assertion(error),
        lifecycle::TerminalAssertionError::Operation(error) => to_py_err(error),
        other => PyValueError::new_err(other.to_string()),
    };
}

fn profile(value: &str) -> PyResult<core::SnapshotProfile> {
    return serde_json::from_value(serde_json::json!(value))
        .map_err(|error| PyValueError::new_err(error.to_string()));
}
/// Report a successful positive expectation inside negation with shared diagnostics.
///
/// # Errors
/// Always returns a typed expectation mismatch.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn fail_negation() -> PyResult<()> {
    return Err(mismatch(core::negation_mismatch()));
}

/// Normalize this exact capture, without observing or retrying.
///
/// # Errors
/// Rejects invalid options and insufficient frame detail.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (frame, profile_name = "text", include_cursor = false, include_modes = false))]
pub fn normalized_snapshot(
    frame: &PyFrame,
    profile_name: &str,
    include_cursor: bool,
    include_modes: bool,
) -> PyResult<String> {
    return core::normalized_snapshot(
        &frame.frame,
        profile(profile_name)?,
        ScreenSnapshotOptions {
            include_cursor,
            include_modes,
        },
    )
    .map_err(assertion);
}
/// Normalize a separate masked cells comparison. Masks are not redaction.
///
/// # Errors
/// Rejects invalid/split-wide masks and incomplete cell maps.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn masked_snapshot(
    frame: &PyFrame,
    options: Arg<core::MaskedSnapshotOptions>,
) -> PyResult<String> {
    let options = options.0;
    return core::masked_snapshot(&frame.frame, &options).map_err(assertion);
}
/// Compare/create an unmasked file baseline. Only explicit update permits writes.
///
/// # Errors
/// Returns a mismatch for missing/different baselines, operational errors otherwise.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (frame, path, profile_name = "text", include_cursor = false, include_modes = false, update = false))]
pub fn assert_snapshot_file(
    py: Python<'_>,
    frame: &PyFrame,
    path: String,
    profile_name: &str,
    include_cursor: bool,
    include_modes: bool,
    update: bool,
) -> PyResult<()> {
    let profile = profile(profile_name)?;
    return py.detach(|| {
        lifecycle::assert_snapshot_file(
            &frame.frame,
            Path::new(&path),
            profile,
            ScreenSnapshotOptions {
                include_cursor,
                include_modes,
            },
            update,
        )
        .map_err(terminal_error)
    });
}
/// Compare/create a separate masked baseline. Masks are not redaction.
///
/// # Errors
/// Validates before writes; only comparison mismatches are negatable.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (frame, path, options, update = false))]
pub fn assert_masked_snapshot_file(
    py: Python<'_>,
    frame: &PyFrame,
    path: String,
    options: Arg<core::MaskedSnapshotOptions>,
    update: bool,
) -> PyResult<()> {
    let options = options.0;
    return py.detach(|| {
        lifecycle::assert_masked_snapshot_file(&frame.frame, Path::new(&path), &options, update)
            .map_err(terminal_error)
    });
}

/// Shared Rust immutable `assert_screen_text` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_screen_text(frame: &PyFrame, expected: String) -> PyResult<()> {
    return core::assert_screen_text(&frame.frame, &expected).map_err(assertion);
}

/// Shared Rust immutable `assert_cursor` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_cursor(frame: &PyFrame, expected: Arg<CursorState>) -> PyResult<()> {
    return core::assert_cursor(&frame.frame, &expected.0).map_err(mismatch);
}

/// Shared Rust immutable `assert_no_wrapped_rows` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_no_wrapped_rows(frame: &PyFrame) -> PyResult<()> {
    return core::assert_no_wrapped_rows(&frame.frame).map_err(assertion);
}

/// Shared Rust immutable `assert_neutral_region` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_neutral_region(frame: &PyFrame, region: Arg<ScreenRegion>) -> PyResult<()> {
    return core::assert_neutral_region(&frame.frame, region.0).map_err(assertion);
}

/// Shared Rust immutable `assert_no_presented_transient` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_no_presented_transient(frame: &PyFrame) -> PyResult<()> {
    return core::assert_no_presented_transient(&frame.frame).map_err(mismatch);
}

/// Shared Rust immutable `assert_exists` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_exists(result: &PyQueryResult) -> PyResult<()> {
    return core::assert_exists(&result.inner).map_err(mismatch);
}

/// Shared Rust immutable `assert_not_exists` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_not_exists(result: &PyQueryResult) -> PyResult<()> {
    return core::assert_not_exists(&result.inner).map_err(mismatch);
}

/// Shared Rust immutable `assert_count` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_count(result: &PyQueryResult, expected: usize) -> PyResult<()> {
    return core::assert_count(&result.inner, expected).map_err(mismatch);
}

/// Shared Rust immutable `assert_match_text` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_match_text(selected: &PyScreenMatch, expected: String) -> PyResult<()> {
    return core::assert_match_text(&selected.inner, &expected).map_err(mismatch);
}

/// Shared Rust immutable `assert_match_style` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only expectation mismatches or non-negatable validation/query errors.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn assert_match_style(selected: &PyScreenMatch, expected: Arg<StyleFilter>) -> PyResult<()> {
    return core::assert_match_style(&selected.inner, &expected.0).map_err(assertion);
}

/// Finalize owned recordings through the shared Rust lifecycle engine, releasing the GIL.
///
/// # Errors
/// Rejects invalid outcomes, policies or timeouts before transferring session ownership.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (recordings, root, outcome, policy, timeout_secs = None, retain_request = None))]
pub fn finalize_recordings(
    py: Python<'_>,
    recordings: Vec<PyRecording>,
    root: String,
    outcome: Arg<lifecycle::TestOutcome>,
    policy: Arg<lifecycle::EvidencePolicy>,
    timeout_secs: Option<f64>,
    retain_request: Option<crate::locator::PyCancelToken>,
) -> PyResult<Model<lifecycle::FinalizationReport>> {
    let outcome = outcome.0;
    let policy = policy.0;
    let timeout = crate::parse_timeout(timeout_secs, Duration::from_secs(10))?;
    if recordings.iter().any(|rec| {
        !Path::new(&rec.path).starts_with(Path::new(&root))
            || Path::new(&rec.path) == Path::new(&root)
    }) {
        return Err(PyValueError::new_err(
            "owned recording is outside the test root".to_owned(),
        ));
    }
    let report = py.detach(move || {
        let mut recordings = recordings;
        recordings.sort_by(|left, right| left.path.cmp(&right.path));
        recordings.dedup_by(|left, right| left.path == right.path);
        // Lock cached reports in path order across the entire operation. Concurrent/layered
        // wrappers cannot take a second ownership, capture duplicate artifacts or finalize twice.
        let mut reports: Vec<_> = recordings
            .iter()
            .map(|rec| rec.finalization.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        if let Some(first) = reports.first().and_then(|report| report.as_ref())
            && first.root == Path::new(&root)
            && first.policy == policy
            && reports.iter().all(|cached| cached.as_ref() == Some(first))
        {
            return first.report.clone();
        }
        let sessions = recordings
            .iter()
            .zip(&reports)
            .map(|(rec, cached)| {
                if let Some(report) = cached.as_ref() {
                    return lifecycle::OwnedSessionFinalization::Finalized {
                        report: report.report.clone(),
                        path: rec.path.clone().into(),
                    };
                }
                let owned = rec.session.lock().unwrap_or_else(|e| e.into_inner()).take();
                return lifecycle::OwnedSessionFinalization::Open(Ok(
                    owned.unwrap_or_else(|| rec.lifecycle.lifecycle_observer())
                ));
            })
            .collect();
        let report = lifecycle::finalize_owned_sessions(
            sessions,
            Path::new(&root),
            outcome,
            policy,
            timeout,
            retain_request.as_ref().map(|token| &token.inner),
        );
        for cached in &mut reports {
            **cached = Some(lifecycle::CachedFinalization {
                root: root.clone().into(),
                policy,
                report: report.clone(),
            });
        }
        return report;
    });
    return Ok(Model(report));
}

/// Register the runner-neutral testing functions.
///
/// # Errors
/// Propagates module registration failures.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(fail_negation, m)?)?;
    m.add_function(wrap_pyfunction!(normalized_snapshot, m)?)?;
    m.add_function(wrap_pyfunction!(masked_snapshot, m)?)?;
    m.add_function(wrap_pyfunction!(assert_snapshot_file, m)?)?;
    m.add_function(wrap_pyfunction!(assert_masked_snapshot_file, m)?)?;
    m.add_function(wrap_pyfunction!(assert_screen_text, m)?)?;
    m.add_function(wrap_pyfunction!(assert_cursor, m)?)?;
    m.add_function(wrap_pyfunction!(assert_no_wrapped_rows, m)?)?;
    m.add_function(wrap_pyfunction!(assert_neutral_region, m)?)?;
    m.add_function(wrap_pyfunction!(assert_no_presented_transient, m)?)?;
    m.add_function(wrap_pyfunction!(assert_exists, m)?)?;
    m.add_function(wrap_pyfunction!(assert_not_exists, m)?)?;
    m.add_function(wrap_pyfunction!(assert_count, m)?)?;
    m.add_function(wrap_pyfunction!(assert_match_text, m)?)?;
    m.add_function(wrap_pyfunction!(assert_match_style, m)?)?;
    m.add_function(wrap_pyfunction!(finalize_recordings, m)?)?;

    return Ok(());
}
