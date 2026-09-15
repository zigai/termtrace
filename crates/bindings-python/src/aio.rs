//! Asyncio bridge for Termtrace using pyo3-async-runtimes.

use crate::actions::PyActionBuilder;
use crate::errors::to_py_err;
use crate::recording::PyRecording;
use crate::screen::{PyResolvedState, PyScreen};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use std::time::Duration;
use termtrace_sdk::screen::frame_text;

/// Asynchronously wait for text pattern on a screen.
#[pyfunction]
#[pyo3(signature = (screen, pattern, timeout_secs = None))]
pub fn wait_for_text_async<'py>(
    py: Python<'py>,
    screen: PyScreen,
    pattern: String,
    timeout_secs: Option<f64>,
) -> PyResult<Bound<'py, PyAny>> {
    let terminal = screen.terminal.clone();
    let path = screen.session_path;
    let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));

    future_into_py(py, async move {
        let outcome = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let s = session.screen();
            let outcome = s.wait_for_text(&pattern, timeout).map_err(to_py_err)?;
            if !outcome.matched {
                return Err(crate::errors::TimeoutError::new_err(format!(
                    "timed out waiting for pattern: '{pattern}'"
                )));
            }
            Ok(outcome)
        })
        .await
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))??;

        let text = frame_text(&outcome.frame);
        Ok(PyResolvedState {
            hash: outcome.frame.state_hash,
            text,
            cursor_row: outcome.frame.cursor.row.unwrap_or(0),
            cursor_col: outcome.frame.cursor.column.unwrap_or(0),
        })
    })
}

/// Asynchronously wait for screen to become idle.
#[pyfunction]
#[pyo3(signature = (screen, timeout_secs = None))]
pub fn wait_for_idle_async<'py>(
    py: Python<'py>,
    screen: PyScreen,
    timeout_secs: Option<f64>,
) -> PyResult<Bound<'py, PyAny>> {
    let terminal = screen.terminal.clone();
    let path = screen.session_path;
    let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));

    future_into_py(py, async move {
        let capture = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let s = session.screen();
            s.wait_for_idle(timeout).map_err(to_py_err)
        })
        .await
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))??;

        let text = frame_text(&capture.frame);
        Ok(PyResolvedState {
            hash: capture.frame.state_hash,
            text,
            cursor_row: capture.frame.cursor.row.unwrap_or(0),
            cursor_col: capture.frame.cursor.column.unwrap_or(0),
        })
    })
}

/// Asynchronously execute an action sequence.
#[pyfunction]
pub fn execute_action_async<'py>(
    py: Python<'py>,
    mut action: PyRefMut<'_, PyActionBuilder>,
) -> PyResult<Bound<'py, PyAny>> {
    let terminal = action.terminal.clone();
    let path = action.session_path.clone();
    let atoms = std::mem::take(&mut action.atoms);
    let options = action.options.clone();

    future_into_py(py, async move {
        let result = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            session.act(&atoms, options).map_err(to_py_err)
        })
        .await
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))??;

        if let Some(frame) = result.frame {
            let text = frame_text(&frame);
            Ok(Some(PyResolvedState {
                hash: frame.state_hash,
                text,
                cursor_row: frame.cursor.row.unwrap_or(0),
                cursor_col: frame.cursor.column.unwrap_or(0),
            }))
        } else {
            Ok(None)
        }
    })
}

/// Asynchronously stop and finalize a recording.
#[pyfunction]
pub fn stop_recording_async<'py>(
    py: Python<'py>,
    recording: PyRecording,
) -> PyResult<Bound<'py, PyAny>> {
    future_into_py(py, async move {
        let mut session_opt = {
            let mut guard = recording.session.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };

        if let Some(mut session) = session_opt.take() {
            tokio::task::spawn_blocking(move || {
                let _ = session.stop(Some(Duration::from_secs(5)));
                Ok(())
            })
            .await
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
        } else {
            Ok(())
        }
    })
}
