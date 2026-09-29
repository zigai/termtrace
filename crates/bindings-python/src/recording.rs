//! RAII Recording owner and context manager for Python.

use crate::actions::PyActionBuilder;
use crate::errors::to_py_err;
use crate::screen::PyScreen;
use crate::session::PySession;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::defaults;
use termtrace_sdk::session::StopRequest;

impl PyRecording {
    fn take(&self) -> Option<Session> {
        return self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take();
    }

    fn restore(&self, session: Session) {
        *self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner()) = Some(session);
    }
}

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
    ///
    /// # Errors
    ///
    /// Raises as `PyRecording::stop` with the default timeout.
    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    pub fn __exit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        self.stop(py, None)
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
    pub fn actions(&self) -> PyResult<PyActionBuilder> {
        if !self.is_active() {
            return Err(PyRuntimeError::new_err(
                "recording has already been stopped or detached",
            ));
        }
        Ok(PyActionBuilder::new(
            self.terminal.clone(),
            self.path.clone(),
        ))
    }

    /// Get a logical session reference.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::pointer_click`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::pointer_scroll`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::pointer_focus`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::save`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::read_output`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::wait_for_output`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::read_events`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::explain`.
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
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::diagnose`.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn diagnose(&self, py: Python<'_>, broad_change_percent: Option<u8>) -> PyResult<String> {
        let sess = self.session()?;

        return sess.diagnose(py, broad_change_percent);
    }

    /// Perform full triage analysis on the most recent canonical action.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::last_action`.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn last_action(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<String> {
        let sess = self.session()?;

        return sess.last_action(py, broad_change_percent);
    }

    /// Stop the recording session and wait for validated completion evidence.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range, and
    /// `TermtraceError` or a subclass when the recorder does not stop with validated completion in
    /// time, leaving the recording attached.
    #[pyo3(signature = (timeout_secs = None))]
    pub fn stop(&self, py: Python<'_>, timeout_secs: Option<f64>) -> PyResult<()> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::STOP_TIMEOUT)?;
        let Some(mut session) = self.take() else {
            return Ok(());
        };

        let (session, result) = py.detach(move || {
            let result = session.stop(StopRequest::default(), Some(timeout));
            return (session, result);
        });

        if let Err(error) = result {
            self.restore(session);
            return Err(to_py_err(error));
        }

        return Ok(());
    }

    /// Forcefully terminate and reap the recorded process group and owned recorder.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the process group or recorder cannot be
    /// terminated and reaped, leaving the recording attached.
    pub fn force_close(&self) -> PyResult<()> {
        let Some(mut session) = self.take() else {
            return Ok(());
        };

        if let Err(error) = session.force_close() {
            self.restore(session);
            return Err(to_py_err(error));
        }

        return Ok(());
    }

    /// Detach the recording so it continues running in the background.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
    pub fn detach(&self) -> PyResult<()> {
        let session = self.take().ok_or_else(|| {
            return PyRuntimeError::new_err("recording has already been stopped or detached");
        })?;
        let _path = session.detach();

        return Ok(());
    }
}
