//! Original-execution recordings for Python.

use crate::actions::request_id_arg;
use crate::errors::{mutation_err, to_py_err};
use crate::host::{Arg, Model};
use crate::locator::PyCancelToken;
use crate::session::PySession;
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::OriginalLaunchOptions;
use termtrace_sdk::defaults;
use termtrace_sdk::session::OriginalCaptureStatus;
use termtrace_sdk::session::{
    OriginalLaunchSpec, OriginalRecording, OriginalSession, SessionTarget, StopRequest,
};
use termtrace_sdk::{MutationGuard, Uuid};

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Original launch specification for already host-decoded options.
///
/// # Errors
///
/// Raises `ValueError` for unknown or malformed options and `TermtraceError` for conflicting
/// geometry or an unsupported rate.
pub fn original_launch_spec(
    command: Vec<String>,
    options: Option<Arg<OriginalLaunchOptions>>,
) -> PyResult<OriginalLaunchSpec> {
    let options = options.map(|options| return options.0).unwrap_or_default();

    return options
        .to_spec(command.into_iter().map(Into::into).collect())
        .map_err(to_py_err);
}

/// Start `spec`, mirroring `cancel` into the start's cancellation flag until it returns.
///
/// # Errors
///
/// Raises `OriginalCaptureFailedError` when acquisition fails or is cancelled, including a
/// cancellation observed after the session became live, and `TermtraceError` otherwise.
pub fn start_original(
    py: Python<'_>,
    terminal: Terminal,
    spec: OriginalLaunchSpec,
    cancel: Option<PyCancelToken>,
) -> PyResult<PyOriginalRecording> {
    py.detach(move || {
        let cancelled = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let late = cancel.clone();
        let bridge = cancel.map(|token| {
            let cancelled = Arc::clone(&cancelled);
            let done = Arc::clone(&done);

            return thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if token.inner.is_cancelled() {
                        cancelled.store(true, Ordering::Release);

                        return;
                    }

                    thread::sleep(CANCEL_POLL_INTERVAL);
                }
            });
        });

        let started = terminal.start_original(&spec, &cancelled);
        done.store(true, Ordering::Release);

        if let Some(bridge) = bridge {
            let _joined = bridge.join();
        }

        let original = started.map_err(to_py_err)?;
        // A cancellation the bridge had not forwarded yet still owns the late recording.
        if cancelled.load(Ordering::Acquire) || late.is_some_and(|token| token.inner.is_cancelled())
        {
            return Err(to_py_err(original.cancel()));
        }

        Ok(PyOriginalRecording::new(terminal, original))
    })
}

/// An original recording: a canonical session whose terminal pixels are captured while it runs.
#[gen_stub_pyclass]
#[pyclass(name = "OriginalRecording", from_py_object)]
#[derive(Clone)]
pub struct PyOriginalRecording {
    terminal: Terminal,
    original: Arc<Mutex<Option<OriginalSession>>>,
    session_id: String,
    path: String,
    canonical: PathBuf,
}

fn fs_path(value: &Bound<'_, PyAny>) -> PyResult<PathBuf> {
    let bytes: Vec<u8> = value
        .py()
        .import("os")?
        .call_method1("fsencode", (value,))?
        .extract()?;

    return Ok(PathBuf::from(OsString::from_vec(bytes)));
}

/// `str` and `bytes` follow the `path:`/`name:` reference rules; an `os.PathLike` is always an
/// exact path.
pub(crate) fn session_target(target: &Bound<'_, PyAny>) -> PyResult<SessionTarget> {
    if let Ok(text) = target.extract::<String>() {
        return SessionTarget::parse(OsStr::new(&text)).map_err(to_py_err);
    }

    if let Ok(bytes) = target.cast::<PyBytes>() {
        return SessionTarget::parse(OsStr::from_bytes(bytes.as_bytes())).map_err(to_py_err);
    }

    if target.hasattr("__fspath__")? {
        return fs_path(target).map(SessionTarget::Path);
    }
    return Err(PyTypeError::new_err(
        "original target must be str, bytes or os.PathLike",
    ));
}

pub(crate) fn state_root(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<PathBuf>> {
    return value.map(fs_path).transpose();
}

impl PyOriginalRecording {
    pub(crate) fn new(terminal: Terminal, original: OriginalSession) -> Self {
        return Self {
            session_id: original.session().session_id().to_string(),
            path: original.session().path().display().to_string(),
            canonical: original.session().path().to_path_buf(),
            terminal,
            original: Arc::new(Mutex::new(Some(original))),
        };
    }

    fn take(&self) -> PyResult<OriginalSession> {
        return self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take()
            .ok_or_else(|| {
                return PyRuntimeError::new_err(
                    "original recording has already finished or been detached",
                );
            });
    }

    fn restore(&self, original: OriginalSession) {
        *self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner()) = Some(original);
    }

    // A capture that failed or timed out keeps its owner, so it can be waited for again or
    // force-closed; a published or failed capture whose supervisor exited has nothing left.
    fn settle(
        &self,
        original: OriginalSession,
        result: termtrace_sdk::Result<OriginalRecording>,
        request_id: Option<Uuid>,
    ) -> PyResult<Model<OriginalRecording>> {
        return match result {
            Ok(recording) => Ok(Model(recording)),
            Err(error) => {
                let finished = matches!(error, termtrace_sdk::Error::OriginalCaptureFailed { .. });
                if !finished {
                    self.restore(original);
                }

                // Capture lifecycle failures are management facts, not the stop request's.
                let lifecycle = matches!(
                    error,
                    termtrace_sdk::Error::OriginalCaptureFailed { .. }
                        | termtrace_sdk::Error::OriginalFinalizationTimeout { .. }
                );

                Err(match request_id {
                    Some(request_id) if !lifecycle => mutation_err(error, request_id),
                    _ => to_py_err(error),
                })
            }
        };
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyOriginalRecording {
    /// Unique session identifier.
    #[getter]
    pub fn session_id(&self) -> &str {
        return &self.session_id;
    }

    /// Filesystem path to the canonical session directory.
    #[getter]
    pub fn path(&self) -> &str {
        return &self.path;
    }

    /// Whether this handle still owns the recording.
    pub fn is_active(&self) -> bool {
        return self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .is_some();
    }

    /// Exact canonical session directory bytes.
    #[getter]
    pub fn path_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        return PyBytes::new(py, self.canonical.as_os_str().as_bytes());
    }

    /// Live control and observation of the canonical session; resizes move the captured window.
    pub fn session(&self) -> PySession {
        return PySession {
            terminal: self.terminal.clone(),
            session_path: self.canonical.clone(),
            session_id: self.session_id.clone(),
        };
    }

    /// Supervisor status, with `abandoned` derived when the supervisor is gone.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` when the status sidecar is missing or invalid.
    pub fn capture_status(&self, py: Python<'_>) -> PyResult<Model<OriginalCaptureStatus>> {
        let path = self.canonical.clone();
        let status = py
            .detach(move || return termtrace_sdk::session::original_capture_status(&path))
            .map_err(to_py_err)?;
        return Ok(Model(status));
    }

    /// Stop the application and wait until the capture is published.
    ///
    /// Returns `completion`, `canonical_success` and `capture`, the published media artifact.
    ///
    /// # Errors
    ///
    /// Raises `OriginalCaptureFailedError` when the capture did not publish,
    /// `OriginalFinalizationTimeoutError` when it was still finalizing at the deadline (the
    /// recording stays owned), and stop failures with `request_id`.
    #[pyo3(signature = (timeout_ms = None, request_id = None))]
    pub fn finish(
        &self,
        py: Python<'_>,
        timeout_ms: Option<u64>,
        request_id: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Model<OriginalRecording>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, Duration::from_millis);
        let request_id = request_id_arg(request_id)?.unwrap_or_else(Uuid::now_v7);
        let mut original = self.take()?;
        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };
        let (original, result) = py.detach(move || {
            let result = original.finish(request, timeout);

            return (original, result);
        });

        return self.settle(original, result, Some(request_id));
    }

    /// Wait without stopping until the application exits and the capture is published.
    ///
    /// # Errors
    ///
    /// As `finish`, without a stop request.
    #[pyo3(signature = (timeout_ms = None))]
    pub fn wait_finished(
        &self,
        py: Python<'_>,
        timeout_ms: Option<u64>,
    ) -> PyResult<Model<OriginalRecording>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, Duration::from_millis);
        let mut original = self.take()?;
        let (original, result) = py.detach(move || {
            let result = original.wait_finished(timeout);

            return (original, result);
        });

        return self.settle(original, result, None);
    }

    /// Kill the application's process group and the capture supervisor; the capture is
    /// abandoned. A no-op once the recording finished or was detached.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` when the processes cannot be terminated and reaped, leaving
    /// the recording owned.
    pub fn force_close(&self, py: Python<'_>) -> PyResult<()> {
        let Ok(mut original) = self.take() else {
            return Ok(());
        };
        let (original, result) = py.detach(move || {
            let result = original.force_close();

            return (original, result);
        });

        if let Err(error) = result {
            self.restore(original);

            return Err(to_py_err(error));
        }

        return Ok(());
    }

    /// Stop managing the recording without stopping it.
    ///
    /// # Errors
    ///
    /// Raises `RuntimeError` when the recording already finished or was detached.
    pub fn detach(&self) -> PyResult<()> {
        let _path = self.take()?.detach();

        return Ok(());
    }
}
