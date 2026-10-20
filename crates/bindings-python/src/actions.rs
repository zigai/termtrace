//! Fluent action builder for executing state-guarded terminal input in Python.

use crate::errors::to_py_err;
use crate::screen::PyResolvedState;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::time::Duration;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults;
use termtrace_sdk::input::{ActRequest, KeyChord};
use termtrace_sdk::screen::ScreenPredicate;

/// Parse one key chord with the shared grammar, e.g. `Enter`, `ctrl+c`, `ctrl+shift+F5`.
///
/// # Errors
///
/// Returns `ValueError` when `key` is not a valid key chord or names an unknown key.
pub fn parse_key(key: &str) -> PyResult<KeyChord> {
    return KeyChord::parse(key)
        .map_err(|error| return PyValueError::new_err(error.to_string()))?
        .ok_or_else(|| {
            return PyValueError::new_err(format!(
                "unknown key name: '{key}'; send plain characters as text"
            ));
        });
}

/// Fluent builder for atomic, state-guarded input sequences.
#[pyclass(name = "ActionBuilder")]
pub struct PyActionBuilder {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
    pub(crate) request: ActRequest,
}

impl PyActionBuilder {
    /// Start an empty action against one session.
    pub(crate) fn new(terminal: Terminal, session_path: String) -> Self {
        return Self {
            terminal,
            session_path,
            request: ActRequest::new(),
        };
    }

    /// Apply the action, returning its terminating frame when one was projected.
    pub(crate) fn blocking_execute(
        terminal: &Terminal,
        session_path: &str,
        request: ActRequest,
    ) -> PyResult<Option<PyResolvedState>> {
        let outcome = terminal
            .attach(session_path)
            .and_then(|session| return session.act(request))
            .map_err(to_py_err)?;

        return Ok(outcome.frame().map(PyResolvedState::from_frame));
    }
}

#[pymethods]
impl PyActionBuilder {
    /// Enforce that the terminal must be at the specified state hash before executing.
    pub fn if_state(mut slf: PyRefMut<'_, Self>, state_hash: String) -> PyRefMut<'_, Self> {
        slf.request.guard.if_state = Some(state_hash);
        return slf;
    }

    /// Append UTF-8 text to send to the terminal.
    pub fn send_text(mut slf: PyRefMut<'_, Self>, text: String) -> PyRefMut<'_, Self> {
        slf.request = std::mem::take(&mut slf.request).text(text);
        return slf;
    }

    /// Append a key chord (e.g. 'Enter', 'ctrl+c', 'ctrl+shift+F5').
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `key` is not a valid key chord or names an unknown key.
    pub fn send_key(mut slf: PyRefMut<'_, Self>, key: String) -> PyResult<PyRefMut<'_, Self>> {
        let chord = parse_key(&key)?;
        slf.request = std::mem::take(&mut slf.request).key(chord);
        return Ok(slf);
    }

    /// Append exact raw bytes to send to the terminal.
    pub fn send_bytes(mut slf: PyRefMut<'_, Self>, bytes: Vec<u8>) -> PyRefMut<'_, Self> {
        slf.request = std::mem::take(&mut slf.request).bytes(bytes);
        return slf;
    }

    /// Send bracketed paste text.
    pub fn paste(mut slf: PyRefMut<'_, Self>, text: String) -> PyRefMut<'_, Self> {
        slf.request = std::mem::take(&mut slf.request).paste(text);
        return slf;
    }

    /// Set delay pacing between consecutive action atoms in milliseconds; paced text is typed
    /// one character at a time.
    pub fn pace(mut slf: PyRefMut<'_, Self>, delay_ms: u64) -> PyRefMut<'_, Self> {
        slf.request.pace = Duration::from_millis(delay_ms);

        return slf;
    }

    /// Wait until the screen contains the specified pattern after applying input.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn until_screen(
        mut slf: PyRefMut<'_, Self>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyRefMut<'_, Self>> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;
        slf.request = std::mem::take(&mut slf.request)
            .until_screen(ScreenPredicate::Contains { text: pattern }, timeout);
        return Ok(slf);
    }

    /// Wait until the screen changes after applying input and then stays unchanged.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range.
    #[pyo3(signature = (stable_for_ms = None, timeout_secs = None))]
    pub fn until_idle(
        mut slf: PyRefMut<'_, Self>,
        stable_for_ms: Option<u64>,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyRefMut<'_, Self>> {
        let stable_for = stable_for_ms.map_or(defaults::IDLE_STABLE_FOR, Duration::from_millis);
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;
        slf.request = std::mem::take(&mut slf.request).until_idle(stable_for, timeout);
        return Ok(slf);
    }

    /// Execute the action sequence and return the terminating screen frame.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the recorder
    /// rejects or cannot complete the queued actions.
    pub fn execute(&mut self, py: Python<'_>) -> PyResult<Option<PyResolvedState>> {
        let request = std::mem::take(&mut self.request);

        return py.detach(|| {
            return Self::blocking_execute(&self.terminal, &self.session_path, request);
        });
    }
}
