//! RAII Recording owner and context manager for Python.

use crate::actions::{request_id_arg, PyActionBuilder};
use crate::control::{self, PyPointer};
use crate::errors::{mutation_err, to_py_err};
use crate::host::{Arg, Model};
use crate::screen::PyScreen;
use crate::session::PySession;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termtrace_model::artifact::save::SaveOutcome;
use termtrace_model::execution::event::Event;
use termtrace_model::execution::session::SessionCompletion;
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_model::terminal::analysis::{DiagnoseReport, ExplainReport, LastActionAnalysis};
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::contracts::StopReport;
use termtrace_sdk::contracts::{ConditionSpec, SendStepSpec};
use termtrace_sdk::defaults;
use termtrace_sdk::screen::{FrameDetail, ScreenRegion};
use termtrace_sdk::session::{
    ControlAcknowledgement, MarkOutcome, SessionStatus, StopOutcome, StopRequest,
};
use termtrace_sdk::testing::{EvidencePolicy, TestOutcome};
use termtrace_sdk::{MutationGuard, Uuid};

impl PyRecording {
    fn take(&self) -> Option<Session> {
        return self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take();
    }

    fn restore(&self, session: Session) {
        if session.lifecycle_finished() || session.lifecycle_force_requested() {
            return;
        }

        *self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner()) = Some(session);
    }

    fn stop_request(
        &self,
        py: Python<'_>,
        request_id: Uuid,
        wait: Option<Duration>,
    ) -> PyResult<Option<StopOutcome>> {
        let Some(mut session) = self.take() else {
            return Ok(None);
        };

        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };

        let (session, result) = py.detach(move || {
            let result = session.stop(request, wait);

            return (session, result);
        });

        return match result {
            Ok(completed @ StopOutcome::Completed { .. }) => Ok(Some(completed)),
            Ok(requested @ StopOutcome::Requested { .. }) => {
                self.restore(session);

                Ok(Some(requested))
            }
            Err(error) => {
                self.restore(session);

                Err(mutation_err(error, request_id))
            }
        };
    }

    fn inactive() -> PyErr {
        return PyRuntimeError::new_err("recording has already been stopped or detached");
    }
}

/// An active recording session owning child processes and evidence buffers.
#[gen_stub_pyclass]
#[pyclass(name = "Recording", from_py_object)]
#[derive(Clone)]
pub struct PyRecording {
    pub(crate) terminal: Terminal,
    pub(crate) session: Arc<Mutex<Option<Session>>>,
    pub(crate) session_id: String,
    pub(crate) path: String,
    pub(crate) finalization: Arc<Mutex<Option<termtrace_sdk::testing::CachedFinalization>>>,
    pub(crate) lifecycle: Arc<Session>,
}

#[gen_stub_pymethods]
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
        let root = std::path::Path::new(&self.path)
            .parent()
            .ok_or_else(|| PyRuntimeError::new_err("session has no evidence parent"))?
            .to_string_lossy()
            .into_owned();
        let primary = _exc_val.filter(|error| !error.is_none());
        let report = crate::testing::finalize_recordings(
            py,
            vec![self.clone()],
            root,
            Arg(if primary.is_some() { TestOutcome::Error } else { TestOutcome::Passed }),
            Arg(EvidencePolicy::Manual),
            None,
            None,
        )
        .and_then(|report| return report.into_pyobject(py));
        if let Some(primary) = primary {
            match report {
                Ok(report) => {
                    if primary.hasattr("termtrace").unwrap_or(true)
                        || primary.setattr("termtrace", &report).is_err()
                    {
                        let _logged = py.import("logging").and_then(|logging| {
                            logging.call_method1(
                                "error",
                                ("Termtrace secondary finalization report: %s", &report),
                            )
                        });
                    }
                }
                Err(error) => {
                    let _logged = py.import("logging").and_then(|logging| {
                        logging.call_method1(
                            "error",
                            (
                                "Termtrace finalization failed; evidence retained: %s",
                                error.to_string(),
                            ),
                        )
                    });
                }
            }
            return Ok(());
        }
        let report = report?;
        let failures = report.get_item("secondary_failures")?;
        if !failures.is_empty()? {
            let error = PyRuntimeError::new_err("Termtrace recording cleanup failed");
            error.value(py).setattr("termtrace", report)?;
            return Err(error);
        }
        return Ok(());
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
        guard
            .as_ref()
            .is_some_and(|session| !session.lifecycle_finished())
            || (self.lifecycle.lifecycle_force_requested() && !self.lifecycle.lifecycle_finished())
    }

    /// Explicit application instrumentation; retain the emitter to share a process instance.
    pub fn instrumentation(&self) -> crate::diagnostics::PyInstrumentation {
        return crate::diagnostics::PyInstrumentation::new(crate::host::PathArg(PathBuf::from(
            &self.path,
        )));
    }

    /// Typed analysis operations through the shared service.
    pub fn analysis(&self) -> crate::diagnostics::PyDiagnostics {
        return crate::diagnostics::PyDiagnostics {
            path: PathBuf::from(&self.path),
        };
    }

    /// Typed evidence operations through the shared service.
    pub fn evidence(&self) -> crate::diagnostics::PyEvidence {
        return crate::diagnostics::PyEvidence {
            path: PathBuf::from(&self.path),
        };
    }

    /// Native replay operations through the shared service.
    pub fn native(&self) -> crate::native::PyNative {
        return crate::native::PyNative {
            terminal: self.terminal.clone(),
            path: PathBuf::from(&self.path),
        };
    }

    /// Typed integrity operations through the shared service.
    pub fn integrity(&self) -> crate::diagnostics::PyEvidence {
        return crate::diagnostics::PyEvidence {
            path: PathBuf::from(&self.path),
        };
    }

    /// Source-bound exact PTY output pages.
    pub fn output(&self) -> crate::evidence::PyOutput {
        return crate::evidence::PyOutput {
            path: PathBuf::from(&self.path),
        };
    }

    /// Source-bound filtered canonical event projections.
    pub fn events(&self) -> crate::evidence::PyEvents {
        return crate::evidence::PyEvents {
            path: PathBuf::from(&self.path),
        };
    }

    /// Access screen inspection capabilities.
    ///
    /// # Errors
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
    pub fn screen(&self) -> PyResult<PyScreen> {
        if !self.is_active() {
            return Err(PyRuntimeError::new_err(
                "recording has already been stopped or detached",
            ));
        }
        Ok(PyScreen {
            terminal: self.terminal.clone(),
            session_path: PathBuf::from(&self.path),
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
            PathBuf::from(&self.path),
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
            session_path: PathBuf::from(&self.path),
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

    /// Mode-aware pointer, focus, and text-activation operations with typed results.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached.
    pub fn pointer(&self) -> PyResult<PyPointer> {
        return Ok(self.session()?.pointer());
    }

    /// Resize the child PTY to `cols` by `rows` cells.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::resize`.
    #[pyo3(signature = (cols, rows, *, request_id = None, if_state = None))]
    pub fn resize(
        &self,
        py: Python<'_>,
        cols: u16,
        rows: u16,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<ControlAcknowledgement>> {
        return self.session()?.resize(py, cols, rows, request_id, if_state);
    }

    /// Send one allow-listed signal to the child process group.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::signal`.
    #[pyo3(signature = (signal, *, request_id = None, if_state = None))]
    pub fn signal(
        &self,
        py: Python<'_>,
        signal: Arg<ProcessSignal>,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<ControlAcknowledgement>> {
        return self.session()?.signal(py, signal, request_id, if_state);
    }

    /// Append a named marker to the canonical event stream.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::mark`.
    #[pyo3(signature = (label, *, request_id = None, if_state = None))]
    pub fn mark(
        &self,
        py: Python<'_>,
        label: String,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<MarkOutcome>> {
        return self.session()?.mark(py, label, request_id, if_state);
    }

    /// Apply ordered steps as `PySession::send`.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording has already been stopped or detached, and otherwise
    /// as `PySession::send`.
    #[pyo3(signature = (steps, *, request_id = None, if_state = None, intent = None, pace_ms = None, verbose = false, until = None, detail = None, region = None, report_changes = false))]
    pub fn send<'py>(
        &self,
        py: Python<'py>,
        steps: Vec<Arg<SendStepSpec>>,
        request_id: Option<Bound<'py, PyAny>>,
        if_state: Option<String>,
        intent: Option<String>,
        pace_ms: Option<u64>,
        verbose: bool,
        until: Option<Arg<ConditionSpec>>,
        detail: Option<Arg<FrameDetail>>,
        region: Option<Arg<ScreenRegion>>,
        report_changes: bool,
    ) -> PyResult<Py<PyAny>> {
        return self.session()?.send(
            py,
            steps,
            request_id,
            if_state,
            intent,
            pace_ms,
            verbose,
            until,
            detail,
            region,
            report_changes,
        );
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
    ) -> PyResult<Model<SaveOutcome>> {
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
    ) -> PyResult<Vec<Model<Event>>> {
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
    ) -> PyResult<Model<ExplainReport>> {
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
    pub fn diagnose(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<Model<DiagnoseReport>> {
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
    ) -> PyResult<Model<LastActionAnalysis>> {
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
        let _outcome = self.stop_request(py, Uuid::now_v7(), Some(timeout))?;

        return Ok(());
    }

    /// Ask the recording to end; with `wait_ms`, wait that long for validated completion.
    ///
    /// Returns `{"kind": "requested", ...}` or `{"kind": "completed", ...}`. A failed request
    /// leaves the recording attached; pass `request_id` to retry it with the same identity.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when already stopped or detached, and `TermtraceError` or a subclass
    /// with `request_id` when the stop is rejected or completion is not published in time.
    #[pyo3(signature = (request_id = None, wait_ms = None))]
    pub fn request_stop(
        &self,
        py: Python<'_>,
        request_id: Option<&Bound<'_, PyAny>>,
        wait_ms: Option<u64>,
    ) -> PyResult<Model<StopReport>> {
        let request_id = request_id_arg(request_id)?.unwrap_or_else(Uuid::now_v7);
        let outcome = self
            .stop_request(py, request_id, wait_ms.map(Duration::from_millis))?
            .ok_or_else(Self::inactive)?;

        return Ok(Model(StopReport {
            request_id,
            outcome,
        }));
    }

    /// Wait for validated completion evidence and reap the recorder; returns the completion.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when already stopped or detached, and `TermtraceError` or a subclass
    /// when completion is not published in time, leaving the recording attached.
    #[pyo3(signature = (timeout_ms = None))]
    pub fn wait_for_completion(
        &self,
        py: Python<'_>,
        timeout_ms: Option<u64>,
    ) -> PyResult<Model<SessionCompletion>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, Duration::from_millis);
        let mut session = self.take().ok_or_else(Self::inactive)?;
        let (session, result) = py.detach(move || {
            let result = session.complete(timeout);
            return (session, result);
        });

        return match result {
            Ok(completion) => Ok(Model(completion)),
            Err(error) => {
                self.restore(session);
                Err(to_py_err(error))
            }
        };
    }

    /// Live status `{"kind": "live", ...}` or completion `{"kind": "completed", ...}`, also after
    /// stop.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or read.
    pub fn status(&self, py: Python<'_>) -> PyResult<Model<SessionStatus>> {
        return control::status(py, &self.terminal, &PathBuf::from(&self.path));
    }

    /// Delete this completed canonical session, reaping an owned recorder.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when completion is missing or invalid or the
    /// session cannot be removed durably; an owned recording stays attached.
    pub fn discard_completed(&self, py: Python<'_>) -> PyResult<()> {
        let owned = self.take();
        let attached = owned.is_some();
        let mut session = match owned {
            Some(session) => session,
            None => {
                let terminal = self.terminal.clone();
                let path = self.path.clone();
                py.detach(move || return terminal.attach(path))
                    .map_err(to_py_err)?
            }
        };

        let (session, result) = py.detach(move || {
            let result = session.discard_completed();
            return (session, result);
        });

        if let Err(error) = result {
            if attached {
                self.restore(session);
            }
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
        let mut session = match self.take() {
            Some(session) => session,
            None if self.lifecycle.lifecycle_force_requested() => {
                self.lifecycle.lifecycle_observer()
            }
            None => return Ok(()),
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
