//! RAII Recording owner and context manager for Python.

use crate::actions::PyActionBuilder;
use crate::screen::PyScreen;
use crate::session::PySession;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termtrace_sdk::actions::ActionOptions;
use termtrace_sdk::client::{Session, Terminal};

/// An active recording session owning child processes and evidence buffers.
#[pyclass(name = "Recording", from_py_object)]
#[derive(Clone)]
pub struct PyRecording {
    pub(crate) terminal: Terminal,
    pub(crate) session: Arc<Mutex<Option<Session>>>,
    pub(crate) session_id: String,
    pub(crate) path: String,
}

#[pymethods]
impl PyRecording {
    /// Context manager entry.
    pub fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    /// Context manager exit ensuring child processes are reaped.
    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    pub fn __exit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        self.stop(py)
    }

    /// Unique session identifier.
    #[getter]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Filesystem path to the canonical session directory.
    #[getter]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Check if the recording session is still active.
    pub fn is_active(&self) -> bool {
        let guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        guard.is_some()
    }

    /// Access screen inspection capabilities.
    pub fn screen(&self) -> PyResult<PyScreen> {
        if !self.is_active() {
            return Err(PyRuntimeError::new_err(
                "recording has already been stopped or detached",
            ));
        }
        Ok(PyScreen {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
        })
    }

    /// Create an action builder for input automation.
    pub fn actions(&self) -> PyResult<PyActionBuilder> {
        if !self.is_active() {
            return Err(PyRuntimeError::new_err(
                "recording has already been stopped or detached",
            ));
        }
        Ok(PyActionBuilder {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
            atoms: Vec::new(),
            options: ActionOptions::default(),
        })
    }

    /// Get a logical session reference.
    pub fn session(&self) -> PyResult<PySession> {
        if !self.is_active() {
            return Err(PyRuntimeError::new_err(
                "recording has already been stopped or detached",
            ));
        }
        Ok(PySession {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
            session_id: self.session_id.clone(),
        })
    }
    /// Click mouse button at a terminal cell.
    #[pyo3(signature = (column, row, button = None, strict = None))]
    pub fn pointer_click(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        button: Option<String>,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let sess = self.session()?;

        return sess.pointer_click(py, column, row, button, strict);
    }

    /// Scroll mouse wheel at a terminal cell.
    #[pyo3(signature = (column, row, lines = None, strict = None))]
    pub fn pointer_scroll(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        lines: Option<i16>,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let sess = self.session()?;

        return sess.pointer_scroll(py, column, row, lines, strict);
    }

    /// Report terminal focus state.
    #[pyo3(signature = (focused, strict = None))]
    pub fn pointer_focus(
        &self,
        py: Python<'_>,
        focused: bool,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let sess = self.session()?;

        return sess.pointer_focus(py, focused, strict);
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    #[pyo3(signature = (destination, formats = None, state_hash = None, include_hidden = None))]
    pub fn save(
        &self,
        py: Python<'_>,
        destination: String,
        formats: Option<Vec<String>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> PyResult<String> {
        let sess = self.session()?;

        return sess.save(py, destination, formats, state_hash, include_hidden);
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    #[pyo3(signature = (offset = None, limit = None))]
    pub fn read_output(
        &self,
        py: Python<'_>,
        offset: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<(Vec<u8>, u64, bool)> {
        let sess = self.session()?;

        return sess.read_output(py, offset, limit);
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_output(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<bool> {
        let sess = self.session()?;

        return sess.wait_for_output(py, pattern, timeout_secs);
    }

    /// Read canonical event records as pretty JSON starting at an optional event sequence.
    #[pyo3(signature = (cursor = None, limit = None))]
    pub fn read_events(
        &self,
        py: Python<'_>,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<String> {
        let sess = self.session()?;

        return sess.read_events(py, cursor, limit);
    }

    /// Explain VT and semantic evidence around a target state point.
    #[pyo3(signature = (target_hash = None, context = None))]
    pub fn explain(
        &self,
        py: Python<'_>,
        target_hash: Option<String>,
        context: Option<u64>,
    ) -> PyResult<String> {
        let sess = self.session()?;

        return sess.explain(py, target_hash, context);
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn diagnose(&self, py: Python<'_>, broad_change_percent: Option<u8>) -> PyResult<String> {
        let sess = self.session()?;

        return sess.diagnose(py, broad_change_percent);
    }

    /// Perform full triage analysis on the most recent canonical action.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn last_action(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<String> {
        let sess = self.session()?;

        return sess.last_action(py, broad_change_percent);
    }

    /// Stop the recording session and finalize evidence.
    pub fn stop(&self, py: Python<'_>) -> PyResult<()> {
        let mut session_opt = {
            let mut guard = self
                .session
                .lock()
                .unwrap_or_else(|e| return e.into_inner());
            guard.take()
        };

        let Some(mut session) = session_opt.take() else {
            return Ok(());
        };

        let (session, stop_result) = py.detach(move || {
            let result = session.stop(Some(Duration::from_secs(5)));
            return (session, result);
        });

        if let Err(error) = stop_result {
            let mut guard = self
                .session
                .lock()
                .unwrap_or_else(|e| return e.into_inner());
            *guard = Some(session);
            return Err(PyRuntimeError::new_err(error.to_string()));
        }

        return Ok(());
    }

    /// Forcefully terminate and reap the recorded process group and owned recorder.
    pub fn force_close(&self) -> PyResult<()> {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|e| return e.into_inner());
        let Some(mut session) = guard.take() else {
            return Ok(());
        };

        session
            .force_close()
            .map_err(|e| return PyRuntimeError::new_err(e.to_string()))?;

        return Ok(());
    }

    /// Detach the recording so it continues running in the background.
    pub fn detach(&self) -> PyResult<()> {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|e| return e.into_inner());
        return guard.take().map_or_else(
            || {
                return Err(PyRuntimeError::new_err(
                    "recording has already been stopped or detached",
                ));
            },
            |session| {
                let _ = session.detach();
                return Ok(());
            },
        );
    }
}
