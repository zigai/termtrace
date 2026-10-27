//! Canonical event and output pages for Python.

use crate::{
    errors::to_py_err,
    host::{Arg, Model, PathArg},
};
use pyo3::{exceptions::PyValueError, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::path::PathBuf;
use termtrace_model::{
    execution::paging::{ByteOutputPage, EventFollowPosition, ProjectedEventPage},
    terminal::inspection::WaitOutcome,
};
use termtrace_sdk::{
    events::{EventPageCursor, EventsService},
    output::{OutputPageCursor, OutputService},
};

fn cursor_text<T: serde::Serialize>(cursor: &T) -> PyResult<String> {
    return serde_json::to_string(cursor)
        .map_err(|error| return PyValueError::new_err(error.to_string()));
}

fn cursor_value<T: serde::de::DeserializeOwned>(text: &str) -> PyResult<Model<T>> {
    return serde_json::from_str(text)
        .map(Model)
        .map_err(|error| return PyValueError::new_err(error.to_string()));
}

/// Source-bound canonical PTY-byte pages.
#[gen_stub_pyclass]
#[pyclass(name = "Output", frozen)]
pub struct PyOutput {
    pub(crate) path: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyOutput {
    /// Bind a session path; no filesystem access is performed until a read.
    #[new]
    pub fn new(path: PathArg) -> Self {
        return Self { path: path.0 };
    }

    /// Read one checked finite byte snapshot.
    ///
    /// # Errors
    /// Rejects foreign or modified cursors, invalid limits, invalid complete evidence, and
    /// replacement.
    #[pyo3(signature = (cursor = None, limit = 65536))]
    pub fn page(
        &self,
        py: Python<'_>,
        cursor: Option<Arg<OutputPageCursor>>,
        limit: usize,
    ) -> PyResult<Model<ByteOutputPage>> {
        let cursor = cursor.map(|cursor| return cursor.0);

        return py
            .detach(|| return OutputService::page(&self.path, cursor, limit).map_err(to_py_err))
            .map(Model);
    }

    /// Wait for exact bytes and return the shared structured outcome.
    ///
    /// # Errors
    /// Rejects empty bytes, invalid timeouts, invalid evidence, or completion without a match.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for(
        &self,
        py: Python<'_>,
        pattern: Vec<u8>,
        timeout_secs: Option<f64>,
    ) -> PyResult<Model<WaitOutcome>> {
        let timeout = crate::parse_timeout(timeout_secs, termtrace_sdk::defaults::WAIT_TIMEOUT)?;

        return py
            .detach(|| {
                return OutputService::wait_for_bytes(&self.path, &pattern, timeout)
                    .map_err(to_py_err);
            })
            .map(Model);
    }

    /// Canonical JSON text of a cursor, for storing and resuming later.
    ///
    /// # Errors
    /// Rejects malformed cursors.
    pub fn cursor_to_json(&self, cursor: Arg<OutputPageCursor>) -> PyResult<String> {
        return cursor_text(&cursor.0);
    }

    /// Decode a cursor from its canonical JSON text.
    ///
    /// # Errors
    /// Rejects malformed JSON or cursor shapes.
    pub fn cursor_from_json(&self, text: &str) -> PyResult<Model<OutputPageCursor>> {
        return cursor_value(text);
    }
}

/// Filtered canonical events retaining timestamps, hashes and exact byte summaries.
#[gen_stub_pyclass]
#[pyclass(name = "Events", frozen)]
pub struct PyEvents {
    pub(crate) path: PathBuf,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyEvents {
    /// Bind a session path; no filesystem access is performed until a read.
    #[new]
    pub fn new(path: PathArg) -> Self {
        return Self { path: path.0 };
    }

    /// Read checked event projections; empty filters select every canonical event kind.
    ///
    /// # Errors
    /// Rejects invalid filters, limits, or cursors and invalid or replaced canonical evidence.
    #[pyo3(signature = (cursor = None, limit = 100, metadata_only = false, event_types = None))]
    pub fn page(
        &self,
        py: Python<'_>,
        cursor: Option<Arg<EventPageCursor>>,
        limit: u16,
        metadata_only: bool,
        event_types: Option<Vec<String>>,
    ) -> PyResult<Model<ProjectedEventPage>> {
        let cursor = cursor.map(|cursor| return cursor.0);
        let filter = event_types.unwrap_or_default();

        return py
            .detach(|| {
                return EventsService::page(&self.path, cursor, limit, metadata_only, &filter)
                    .map_err(to_py_err);
            })
            .map(Model);
    }

    /// Ask the original actor for its durable follow positions, or validate completion.
    ///
    /// # Errors
    /// Propagates actor, transport, and canonical completion validation failures.
    #[pyo3(signature = (next_event_seq = 0))]
    pub fn follow(
        &self,
        py: Python<'_>,
        next_event_seq: u64,
    ) -> PyResult<Model<EventFollowPosition>> {
        return py
            .detach(|| {
                let session =
                    termtrace_sdk::Session::attach(&self.path, "termtrace").map_err(to_py_err)?;

                return session
                    .events()
                    .follow(next_event_seq)
                    .map(Into::into)
                    .map_err(to_py_err);
            })
            .map(Model);
    }

    /// Canonical JSON text of a cursor, for storing and resuming later.
    ///
    /// # Errors
    /// Rejects malformed cursors.
    pub fn cursor_to_json(&self, cursor: Arg<EventPageCursor>) -> PyResult<String> {
        return cursor_text(&cursor.0);
    }

    /// Decode a cursor from its canonical JSON text.
    ///
    /// # Errors
    /// Rejects malformed JSON or cursor shapes.
    pub fn cursor_from_json(&self, text: &str) -> PyResult<Model<EventPageCursor>> {
        return cursor_value(text);
    }
}
