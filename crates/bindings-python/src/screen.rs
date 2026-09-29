//! Screen observation and state representations for Python.

use crate::errors::{to_py_err, unsatisfied_wait};
use pyo3::prelude::*;
use std::time::Duration;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults;
use termtrace_sdk::screen::{
    frame_text, ScreenFrame, ScreenPredicate, ScreenProjectionRequest, StableCaptureOptions,
};

/// Immutable snapshot of a resolved presented terminal state.
#[pyclass(name = "ResolvedState", from_py_object)]
#[derive(Clone)]
pub struct PyResolvedState {
    /// Content-addressed semantic state hash.
    #[pyo3(get)]
    pub hash: String,
    /// Normalized plain text representation of the viewport.
    #[pyo3(get)]
    pub text: String,
    /// Zero-indexed cursor row.
    #[pyo3(get)]
    pub cursor_row: u16,
    /// Zero-indexed cursor column.
    #[pyo3(get)]
    pub cursor_col: u16,
}

impl PyResolvedState {
    /// Summarize one exact frame.
    pub fn from_frame(frame: &ScreenFrame) -> Self {
        return Self {
            hash: frame.state_hash.clone(),
            text: frame_text(frame),
            cursor_row: frame.cursor.row.unwrap_or_default(),
            cursor_col: frame.cursor.column.unwrap_or_default(),
        };
    }
}

#[pymethods]
impl PyResolvedState {
    fn __repr__(&self) -> String {
        return format!(
            "ResolvedState(hash='{}', cursor=({}, {}), text_len={})",
            self.hash,
            self.cursor_row,
            self.cursor_col,
            self.text.len()
        );
    }
}

/// Rich screen frame rendered against terminal geometry.
#[pyclass(name = "ScreenFrame", from_py_object)]
#[derive(Clone)]
pub struct PyFrame {
    /// Terminal viewport height in rows.
    #[pyo3(get)]
    pub rows: u16,
    /// Terminal viewport width in columns.
    #[pyo3(get)]
    pub cols: u16,
    /// Plain text string of all rendered rows.
    #[pyo3(get)]
    pub text: String,
}
#[pymethods]
impl PyFrame {
    fn __repr__(&self) -> String {
        return format!(
            "ScreenFrame({}x{}, text_len={})",
            self.cols,
            self.rows,
            self.text.len()
        );
    }
}

/// Screen inspection capability attached to a live or completed session.
#[pyclass(name = "Screen", from_py_object)]
#[derive(Clone)]
pub struct PyScreen {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
}

impl PyScreen {
    fn latest_frame(&self) -> PyResult<ScreenFrame> {
        return self
            .terminal
            .attach(&self.session_path)
            .and_then(|session| return session.frame(ScreenProjectionRequest::default()))
            .map_err(to_py_err);
    }

    /// Wait for literal text on the presented screen; an unmet wait raises `TimeoutError`.
    pub(crate) fn blocking_wait_for_text(
        &self,
        pattern: &str,
        timeout: Duration,
    ) -> PyResult<PyResolvedState> {
        let session = self
            .terminal
            .attach(&self.session_path)
            .map_err(to_py_err)?;

        let predicate = ScreenPredicate::Contains {
            text: pattern.to_owned(),
        };
        let outcome = session
            .wait_screen(predicate, Duration::ZERO, timeout)
            .map_err(to_py_err)?;

        if !outcome.satisfied() {
            return Err(unsatisfied_wait(format!(
                "timed out waiting for pattern: '{pattern}'"
            )));
        }
        return Ok(PyResolvedState::from_frame(&outcome.frame));
    }

    /// Wait until the presented screen stays unchanged for the default idle interval.
    pub(crate) fn blocking_wait_for_idle(&self, timeout: Duration) -> PyResult<PyResolvedState> {
        let capture = self
            .terminal
            .attach(&self.session_path)
            .and_then(|session| {
                return session.capture_stable(StableCaptureOptions {
                    timeout,
                    ..StableCaptureOptions::default()
                });
            })
            .map_err(to_py_err)?;

        return Ok(PyResolvedState::from_frame(&capture.frame));
    }
}

#[pymethods]
impl PyScreen {
    /// Return the current presented screen frame.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the frame
    /// cannot be captured.
    pub fn frame(&self, py: Python<'_>) -> PyResult<PyFrame> {
        return py.detach(|| {
            let frame = self.latest_frame()?;

            return Ok(PyFrame {
                rows: frame.geometry.rows,
                cols: frame.geometry.cols,
                text: frame_text(&frame),
            });
        });
    }

    /// Return the normalized plain text of the latest presented frame.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the frame
    /// cannot be captured.
    pub fn text(&self, py: Python<'_>) -> PyResult<String> {
        return py.detach(|| return Ok(frame_text(&self.latest_frame()?)));
    }

    /// Wait until the presented screen contains the specified text pattern.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
    /// `TimeoutError` when the text is not presented in time, and `TermtraceError` or a subclass
    /// when the session cannot be attached or the wait fails.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_text(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;

        return py.detach(|| return self.blocking_wait_for_text(&pattern, timeout));
    }

    /// Wait until the presented screen remains unchanged for an idle window.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
    /// `TimeoutError` when the screen does not settle in time, and `TermtraceError` or a subclass
    /// when the session cannot be attached or the capture fails.
    #[pyo3(signature = (timeout_secs = None))]
    pub fn wait_for_idle(
        &self,
        py: Python<'_>,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;

        return py.detach(|| return self.blocking_wait_for_idle(timeout));
    }
}
