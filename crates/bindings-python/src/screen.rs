//! Screen observation and state representations for Python.

use crate::errors::to_py_err;
use pyo3::prelude::*;
use std::time::Duration;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::screen::frame_text;
use termtrace_sdk::screen::ScreenProjectionRequest;

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

#[pymethods]
impl PyResolvedState {
    fn __repr__(&self) -> String {
        format!(
            "ResolvedState(hash='{}', cursor=({}, {}), text_len={})",
            self.hash,
            self.cursor_row,
            self.cursor_col,
            self.text.len()
        )
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
        format!(
            "ScreenFrame({}x{}, text_len={})",
            self.cols,
            self.rows,
            self.text.len()
        )
    }
}

/// Screen inspection capability attached to a live or completed session.
#[pyclass(name = "Screen", from_py_object)]
#[derive(Clone)]
pub struct PyScreen {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
}

#[pymethods]
impl PyScreen {
    /// Return the current presented screen frame.
    pub fn frame(&self, py: Python<'_>) -> PyResult<PyFrame> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let screen = session.screen();
            let frame = screen
                .frame(ScreenProjectionRequest::default())
                .map_err(to_py_err)?;
            let text = frame_text(&frame);
            Ok(PyFrame {
                rows: frame.geometry.rows,
                cols: frame.geometry.cols,
                text,
            })
        })
    }

    /// Return the normalized plain text of the latest presented frame.
    pub fn text(&self, py: Python<'_>) -> PyResult<String> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let screen = session.screen();
            let frame = screen
                .frame(ScreenProjectionRequest::default())
                .map_err(to_py_err)?;
            Ok(frame_text(&frame))
        })
    }

    /// Wait until the presented screen contains the specified text pattern.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_text(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let screen = session.screen();
            let outcome = screen.wait_for_text(&pattern, timeout).map_err(to_py_err)?;
            if !outcome.matched {
                return Err(crate::errors::TimeoutError::new_err(format!(
                    "timed out waiting for pattern: '{pattern}'"
                )));
            }
            let text = frame_text(&outcome.frame);
            Ok(PyResolvedState {
                hash: outcome.frame.state_hash.clone(),
                text,
                cursor_row: outcome.frame.cursor.row.unwrap_or(0),
                cursor_col: outcome.frame.cursor.column.unwrap_or(0),
            })
        })
    }

    /// Wait until the presented screen remains unchanged for an idle window.
    #[pyo3(signature = (timeout_secs = None))]
    pub fn wait_for_idle(
        &self,
        py: Python<'_>,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let screen = session.screen();
            let capture = screen.wait_for_idle(timeout).map_err(to_py_err)?;
            let text = frame_text(&capture.frame);
            Ok(PyResolvedState {
                hash: capture.frame.state_hash.clone(),
                text,
                cursor_row: capture.frame.cursor.row.unwrap_or(0),
                cursor_col: capture.frame.cursor.column.unwrap_or(0),
            })
        })
    }
}
