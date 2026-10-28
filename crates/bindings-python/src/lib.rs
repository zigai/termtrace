//! Native Python bindings for Termtrace.

#![allow(
    unsafe_code,
    clippy::single_char_lifetime_names,
    clippy::implicit_return,
    clippy::string_slice,
    clippy::absolute_paths,
    reason = "PyO3 derive and function macros generate FFI code, single-char lifetimes, and implicit returns"
)]

pub mod actions;
pub mod client;
pub mod control;
pub mod diagnostics;
pub mod errors;
pub mod evidence;
pub mod host;
mod input;
pub mod locator;
pub mod native;
pub mod original;
pub mod recording;
pub mod scenario;
pub mod screen;
pub mod session;
pub mod testing;
pub mod video;

use pyo3::prelude::*;
use pyo3_stub_gen::derive::gen_stub_pyfunction;
use std::time::Duration;

/// Parse and validate an optional timeout in seconds, falling back to a shared default.
///
/// # Errors
///
/// Returns `ValueError` when `timeout_secs` is negative, not finite, or too large for a `Duration`.
pub fn parse_timeout(timeout_secs: Option<f64>, default: Duration) -> PyResult<Duration> {
    let Some(secs) = timeout_secs else {
        return Ok(default);
    };
    if !secs.is_finite() || secs < 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "timeout must be a non-negative finite number",
        ));
    }
    return Duration::try_from_secs_f64(secs).map_err(|error| {
        return pyo3::exceptions::PyValueError::new_err(format!(
            "timeout {secs} is out of range: {error}"
        ));
    });
}

/// Returns the Termtrace core version.
#[gen_stub_pyfunction]
#[pyfunction]
fn version() -> &'static str {
    return "0.1.0";
}

pyo3_stub_gen::define_stub_info_gatherer!(stub_info);

/// The `_termtrace` native C-extension module.
#[pymodule]
fn _termtrace(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Classes
    m.add_class::<client::PyTermtrace>()?;
    m.add_class::<evidence::PyEvents>()?;
    m.add_class::<diagnostics::PyDiagnostics>()?;
    m.add_class::<diagnostics::PyEvidence>()?;
    m.add_class::<diagnostics::PyRegistry>()?;
    m.add_class::<diagnostics::PyTmux>()?;
    m.add_class::<diagnostics::PyInstrumentation>()?;
    m.add_class::<diagnostics::PySystem>()?;
    m.add_class::<evidence::PyOutput>()?;
    m.add_class::<native::PyNative>()?;
    m.add_class::<recording::PyRecording>()?;
    m.add_class::<original::PyOriginalRecording>()?;
    m.add_class::<session::PySession>()?;
    m.add_class::<screen::PyScreen>()?;
    m.add_class::<screen::PyFrame>()?;
    m.add_class::<screen::PyResolvedState>()?;
    m.add_class::<locator::PyCancelToken>()?;
    m.add_class::<locator::PyLocatorSpec>()?;
    m.add_class::<locator::PyLocator>()?;
    m.add_class::<locator::PyQueryResult>()?;
    m.add_class::<locator::PyScreenMatch>()?;
    m.add_class::<locator::PyLocatorWaitResult>()?;
    m.add_class::<actions::PyActionBuilder>()?;
    m.add_class::<actions::PyActionResult>()?;
    m.add_class::<control::PyPointer>()?;

    // Functions
    m.add_function(wrap_pyfunction!(version, m)?)?;

    testing::register(m)?;
    video::register(m)?;
    scenario::register(m)?;
    m.add(
        "ExpectationMismatchError",
        m.py().get_type::<errors::ExpectationMismatchError>(),
    )?;

    // Exceptions
    m.add(
        "TermtraceError",
        m.py().get_type::<errors::TermtraceError>(),
    )?;
    m.add(
        "StateGuardFailedError",
        m.py().get_type::<errors::StateGuardFailedError>(),
    )?;
    m.add("TimeoutError", m.py().get_type::<errors::TimeoutError>())?;
    m.add(
        "SessionNotFoundError",
        m.py().get_type::<errors::SessionNotFoundError>(),
    )?;
    m.add(
        "IntegrityError",
        m.py().get_type::<errors::IntegrityError>(),
    )?;
    m.add("QueryError", m.py().get_type::<errors::QueryError>())?;
    m.add(
        "LocatorTimeoutError",
        m.py().get_type::<errors::LocatorTimeoutError>(),
    )?;
    m.add(
        "CancelledError",
        m.py().get_type::<errors::CancelledError>(),
    )?;
    m.add(
        "OriginalCaptureFailedError",
        m.py().get_type::<errors::OriginalCaptureFailedError>(),
    )?;
    m.add(
        "OriginalFinalizationTimeoutError",
        m.py()
            .get_type::<errors::OriginalFinalizationTimeoutError>(),
    )?;

    return Ok(());
}
