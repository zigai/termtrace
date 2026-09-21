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
pub mod aio;
pub mod client;
pub mod errors;
pub mod recording;
pub mod screen;
pub mod session;

use pyo3::prelude::*;
use std::time::Duration;

/// Parse and validate an optional timeout in seconds.
pub fn parse_timeout(timeout_secs: Option<f64>, default_secs: f64) -> PyResult<Duration> {
    let secs = timeout_secs.unwrap_or(default_secs);
    if !secs.is_finite() || secs < 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "timeout must be a non-negative finite number",
        ));
    }
    return Duration::try_from_secs_f64(secs).map_err(|_| {
        return pyo3::exceptions::PyValueError::new_err(
            "timeout must be a non-negative finite number",
        );
    });
}

/// Returns the Termtrace core version.
#[pyfunction]
fn version() -> &'static str {
    return "0.1.0";
}

/// The `_termtrace` native C-extension module.
#[pymodule]
fn _termtrace(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Classes
    m.add_class::<client::PyTermtrace>()?;
    m.add_class::<recording::PyRecording>()?;
    m.add_class::<session::PySession>()?;
    m.add_class::<screen::PyScreen>()?;
    m.add_class::<screen::PyFrame>()?;
    m.add_class::<screen::PyResolvedState>()?;
    m.add_class::<actions::PyActionBuilder>()?;

    // Functions
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(aio::wait_for_text_async, m)?)?;
    m.add_function(wrap_pyfunction!(aio::wait_for_idle_async, m)?)?;
    m.add_function(wrap_pyfunction!(aio::execute_action_async, m)?)?;
    m.add_function(wrap_pyfunction!(aio::stop_recording_async, m)?)?;

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

    return Ok(());
}
