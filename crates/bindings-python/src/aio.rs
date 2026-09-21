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
    let timeout = crate::parse_timeout(timeout_secs, 10.0)?;
    let terminal = screen.terminal.clone();
    let path = screen.session_path;

    return future_into_py(py, async move {
        let outcome = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let s = session.screen();
            let outcome = s.wait_for_text(&pattern, timeout).map_err(to_py_err)?;
            if !outcome.matched {
                return Err(crate::errors::TimeoutError::new_err(format!(
                    "timed out waiting for pattern: '{pattern}'"
                )));
            }
            return Ok(outcome);
        })
        .await
        .map_err(|e| return PyRuntimeError::new_err(e.to_string()))??;

        let text = frame_text(&outcome.frame);
        let cursor_row = outcome.frame.cursor.row.unwrap_or_default();
        let cursor_col = outcome.frame.cursor.column.unwrap_or_default();
        return Ok(PyResolvedState {
            hash: outcome.frame.state_hash,
            text,
            cursor_row,
            cursor_col,
        });
    });
}

/// Asynchronously wait for screen to become idle.
#[pyfunction]
#[pyo3(signature = (screen, timeout_secs = None))]
pub fn wait_for_idle_async<'py>(
    py: Python<'py>,
    screen: PyScreen,
    timeout_secs: Option<f64>,
) -> PyResult<Bound<'py, PyAny>> {
    let timeout = crate::parse_timeout(timeout_secs, 10.0)?;
    let terminal = screen.terminal.clone();
    let path = screen.session_path;

    return future_into_py(py, async move {
        let capture = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let s = session.screen();
            return s.wait_for_idle(timeout).map_err(to_py_err);
        })
        .await
        .map_err(|e| return PyRuntimeError::new_err(e.to_string()))??;

        let text = frame_text(&capture.frame);
        let cursor_row = capture.frame.cursor.row.unwrap_or_default();
        let cursor_col = capture.frame.cursor.column.unwrap_or_default();
        return Ok(PyResolvedState {
            hash: capture.frame.state_hash,
            text,
            cursor_row,
            cursor_col,
        });
    });
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

    return future_into_py(py, async move {
        let result = tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            return session.act(&atoms, options).map_err(to_py_err);
        })
        .await
        .map_err(|e| return PyRuntimeError::new_err(e.to_string()))??;

        if let Some(frame) = result.frame {
            let text = frame_text(&frame);
            let cursor_row = frame.cursor.row.unwrap_or_default();
            let cursor_col = frame.cursor.column.unwrap_or_default();
            return Ok(Some(PyResolvedState {
                hash: frame.state_hash,
                text,
                cursor_row,
                cursor_col,
            }));
        }
        return Ok(None);
    });
}

/// Asynchronously stop and finalize a recording.
#[pyfunction]
pub fn stop_recording_async<'py>(
    py: Python<'py>,
    recording: PyRecording,
) -> PyResult<Bound<'py, PyAny>> {
    return future_into_py(py, async move {
        let mut session_opt = {
            let mut guard = recording
                .session
                .lock()
                .unwrap_or_else(|e| return e.into_inner());
            guard.take()
        };

        let Some(mut session) = session_opt.take() else {
            return Ok(());
        };

        let (session, stop_result) = tokio::task::spawn_blocking(move || {
            let result = session.stop(Some(Duration::from_secs(5)));
            return (session, result);
        })
        .await
        .map_err(|e| return PyRuntimeError::new_err(e.to_string()))?;

        if let Err(error) = stop_result {
            let mut guard = recording
                .session
                .lock()
                .unwrap_or_else(|e| return e.into_inner());
            *guard = Some(session);
            return Err(PyRuntimeError::new_err(error.to_string()));
        }

        return Ok(());
    });
}
