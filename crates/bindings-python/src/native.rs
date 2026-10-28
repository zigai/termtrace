//! Native Ghostty pixels replayed from one canonical session.

use crate::errors::to_py_err;
use crate::host::{Arg, Model, PathArg};
use crate::parse_timeout;
use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use termtrace_model::artifact::visual::CaptureOutcome;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults::NATIVE_REPLAY_TIMEOUT;

/// Native replay operations for one canonical session.
#[gen_stub_pyclass]
#[pyclass(name = "Native", frozen)]
pub struct PyNative {
    pub(crate) terminal: Terminal,
    pub(crate) path: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyNative {
    /// Replay this completed session in Ghostty and capture `point` (default: the latest
    /// presented state) as a PNG in the new `destination` directory, with its manifest and
    /// replay endpoint. `timeout_secs` bounds the replay terminal's startup.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the `native-screenshot` capability is blocked,
    /// the session is live, `point` does not resolve, `destination` exists, or capture fails.
    #[pyo3(signature = (destination, point = None, timeout_secs = None))]
    pub fn screenshot(
        &self,
        py: Python<'_>,
        destination: PathArg,
        point: Option<Arg<StatePoint>>,
        timeout_secs: Option<f64>,
    ) -> PyResult<Model<CaptureOutcome>> {
        let timeout = parse_timeout(timeout_secs, NATIVE_REPLAY_TIMEOUT)?;
        let point = point.map_or(StatePoint::LatestPresented, |point| return point.0);
        let terminal = self.terminal.clone();
        let path = self.path.clone();

        return py.detach(move || {
            let session = terminal.attach(&path).map_err(to_py_err)?;

            return session
                .native()
                .screenshot(
                    &point,
                    &destination.0,
                    None,
                    Arc::new(AtomicBool::new(false)),
                    timeout,
                )
                .map(Model)
                .map_err(to_py_err);
        });
    }
}
