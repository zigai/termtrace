//! Logical session handle and forensic exports for Python.

use crate::actions::PyActionBuilder;
use crate::control::{self, PyPointer};
use crate::errors::to_py_err;
use crate::host::{Arg, Model};
use crate::screen::PyScreen;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use termtrace_model::artifact::bundle::BundleProfile;
use termtrace_model::artifact::save::SaveFormat;
use termtrace_model::artifact::save::SaveOutcome;
use termtrace_model::execution::event::Event;
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::terminal::{
    analysis::{DiagnoseReport, ExplainReport, LastActionAnalysis},
    diff::SemanticDiff,
    timeline::TimelinePage,
};
use termtrace_sdk::analysis::AnalysisService;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::{ConditionSpec, SendStepSpec};
use termtrace_sdk::defaults;
use termtrace_sdk::events::{EventCursor, EventsService};
use termtrace_sdk::output::{OutputCursor, OutputService};
use termtrace_sdk::screen::{FrameDetail, ScreenRegion};
use termtrace_sdk::session::{ControlAcknowledgement, MarkOutcome, SessionStatus};

/// A logical Termtrace session reference.
#[gen_stub_pyclass]
#[pyclass(name = "Session", from_py_object)]
#[derive(Clone)]
pub struct PySession {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
    pub(crate) session_id: String,
}

#[gen_stub_pymethods]
#[pymethods]
impl PySession {
    /// Canonical session identity string.
    #[getter]
    pub fn session_id(&self) -> &str {
        return &self.session_id;
    }

    /// Filesystem path to the session directory, for display when it is not UTF-8.
    #[getter]
    pub fn path(&self) -> String {
        return self.session_path.display().to_string();
    }

    /// Exact session directory bytes.
    #[getter]
    pub fn path_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        return PyBytes::new(py, self.session_path.as_os_str().as_bytes());
    }

    /// Explicit application instrumentation; retain the emitter to share a process instance.
    pub fn instrumentation(&self) -> crate::diagnostics::PyInstrumentation {
        return crate::diagnostics::PyInstrumentation::new(crate::host::PathArg(
            self.session_path.clone(),
        ));
    }

    /// Typed analysis operations through the shared service.
    pub fn analysis(&self) -> crate::diagnostics::PyDiagnostics {
        return crate::diagnostics::PyDiagnostics {
            path: self.session_path.clone(),
        };
    }

    /// Typed evidence operations through the shared service.
    pub fn evidence(&self) -> crate::diagnostics::PyEvidence {
        return crate::diagnostics::PyEvidence {
            path: self.session_path.clone(),
        };
    }

    /// Typed integrity operations through the shared service.
    pub fn integrity(&self) -> crate::diagnostics::PyEvidence {
        return crate::diagnostics::PyEvidence {
            path: self.session_path.clone(),
        };
    }

    /// Source-bound exact PTY output pages.
    pub fn output(&self) -> crate::evidence::PyOutput {
        return crate::evidence::PyOutput {
            path: self.session_path.clone(),
        };
    }

    /// Source-bound filtered canonical event projections.
    pub fn events(&self) -> crate::evidence::PyEvents {
        return crate::evidence::PyEvents {
            path: self.session_path.clone(),
        };
    }

    /// Access screen inspection capabilities.
    pub fn screen(&self) -> PyScreen {
        return PyScreen {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
        };
    }

    /// Create an action builder for input automation.
    pub fn actions(&self) -> PyActionBuilder {
        return PyActionBuilder::new(self.terminal.clone(), self.session_path.clone());
    }

    /// Mode-aware pointer, focus, and text-activation operations with typed results.
    pub fn pointer(&self) -> PyPointer {
        return PyPointer {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
        };
    }

    /// Legacy click adapter over `pointer().click`; returns `None`.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `button` is unknown, and `TermtraceError` or a subclass when the
    /// session cannot be attached or the recorder rejects the click (with `strict`, when the
    /// application does not report mouse input).
    #[pyo3(signature = (column, row, button = None, strict = None))]
    pub fn pointer_click(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        button: Option<String>,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let _result = self.pointer().click(
            py,
            column,
            row,
            button.as_deref(),
            strict.unwrap_or(false),
            None,
            None,
        )?;

        return Ok(());
    }

    /// Legacy scroll adapter over `pointer().scroll`; returns `None`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the recorder
    /// rejects the scroll (with `strict`, when the application does not report mouse input).
    #[pyo3(signature = (column, row, lines = None, strict = None))]
    pub fn pointer_scroll(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        lines: Option<i16>,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let _result = self.pointer().scroll(
            py,
            column,
            row,
            lines.unwrap_or(1),
            strict.unwrap_or(false),
            None,
            None,
        )?;

        return Ok(());
    }

    /// Legacy focus adapter over `pointer().focus`; returns `None`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the recorder
    /// rejects the focus report (with `strict`, when the application does not report focus).
    #[pyo3(signature = (focused, strict = None))]
    pub fn pointer_focus(
        &self,
        py: Python<'_>,
        focused: bool,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let _result = self
            .pointer()
            .focus(py, focused, strict.unwrap_or(false), None, None)?;

        return Ok(());
    }

    /// Resize the child PTY to `cols` by `rows` cells.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
    #[pyo3(signature = (cols, rows, *, request_id = None, if_state = None))]
    pub fn resize(
        &self,
        py: Python<'_>,
        cols: u16,
        rows: u16,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<ControlAcknowledgement>> {
        return control::resize(
            py,
            &self.terminal,
            &self.session_path,
            cols,
            rows,
            request_id,
            if_state,
        );
    }

    /// Send `interrupt`, `terminate`, `hangup`, `user1`, or `user2` to the child process group.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for an unknown signal and `TermtraceError` or a subclass with
    /// `request_id` when the recorder rejects it.
    #[pyo3(signature = (signal, *, request_id = None, if_state = None))]
    pub fn signal(
        &self,
        py: Python<'_>,
        signal: Arg<ProcessSignal>,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<ControlAcknowledgement>> {
        return control::signal(
            py,
            &self.terminal,
            &self.session_path,
            signal,
            request_id,
            if_state,
        );
    }

    /// Append a named marker to the canonical event stream.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
    #[pyo3(signature = (label, *, request_id = None, if_state = None))]
    pub fn mark(
        &self,
        py: Python<'_>,
        label: String,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<MarkOutcome>> {
        return control::mark(
            py,
            &self.terminal,
            &self.session_path,
            label,
            request_id,
            if_state,
        );
    }

    /// Apply ordered steps; each is one mutation whose identity derives from the send's, so
    /// retrying the whole send with its `request_id` replays without duplicates. `if_state`
    /// guards only the first step.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for malformed steps or options, and on a failed step its
    /// `TermtraceError` with `step`, its derived `request_id`, and `send_request_id`; earlier
    /// steps were applied.
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
        let options = control::SendOptions {
            request_id,
            if_state,
            intent,
            pace_ms,
            verbose,
            until,
            detail,
            region,
            report_changes,
        };

        return control::send(py, &self.terminal, &self.session_path, steps, options);
    }

    /// Live status `{"kind": "live", ...}` or completion `{"kind": "completed", ...}`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or read.
    pub fn status(&self, py: Python<'_>) -> PyResult<Model<SessionStatus>> {
        return control::status(py, &self.terminal, &self.session_path);
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when a format name is unknown, and `TermtraceError` or a subclass when
    /// the state cannot be resolved or rendered or the destination cannot be published.
    #[pyo3(signature = (destination, formats = None, state_hash = None, include_hidden = None))]
    pub fn save(
        &self,
        py: Python<'_>,
        destination: String,
        formats: Option<Vec<String>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> PyResult<Model<SaveOutcome>> {
        let session_path = self.session_path.clone();
        let dest = PathBuf::from(destination);
        let save_formats = if let Some(fmts) = formats {
            let mut list = Vec::new();
            for f in fmts {
                match f.to_ascii_lowercase().as_str() {
                    "png" => list.push(SaveFormat::Png),
                    "svg" => list.push(SaveFormat::Svg),
                    "html" => list.push(SaveFormat::Html),
                    "txt" => list.push(SaveFormat::Txt),
                    "json" => list.push(SaveFormat::Json),
                    "deep-json" | "deep_json" => list.push(SaveFormat::DeepJson),
                    "vt" => list.push(SaveFormat::Vt),
                    other => {
                        return Err(pyo3::exceptions::PyValueError::new_err(format!(
                            "unknown save format: '{other}'"
                        )))
                    }
                }
            }

            list
        } else {
            vec![
                SaveFormat::Png,
                SaveFormat::Svg,
                SaveFormat::Txt,
                SaveFormat::Json,
            ]
        };
        let point = state_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let hidden = include_hidden.unwrap_or(false);

        py.detach(move || {
            let outcome = ArtifactService::save(&session_path, &dest, point, &save_formats, hidden)
                .map_err(to_py_err)?;

            return Ok(Model(outcome));
        })
    }

    /// Export session terminal playback as an asciicast v2 file.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the event log cannot be read or the cast cannot
    /// be written.
    pub fn export_asciicast(&self, py: Python<'_>, output_path: String) -> PyResult<()> {
        let session_path = self.session_path.clone();
        let target = PathBuf::from(output_path);

        py.detach(move || {
            ArtifactService::export_asciicast(&session_path, &target).map_err(to_py_err)?;

            return Ok(());
        })
    }

    /// Create a compressed evidence bundle (.tar.zst) for archiving or forensics.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the bundle cannot be created or published.
    pub fn create_bundle(&self, py: Python<'_>, output_path: String) -> PyResult<()> {
        let session_path = self.session_path.clone();
        let target = PathBuf::from(output_path);

        py.detach(move || {
            ArtifactService::create_bundle(&session_path, &target, BundleProfile::Forensic)
                .map_err(to_py_err)?;

            return Ok(());
        })
    }

    /// Compute semantic difference between two presented states and return JSON.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when either state hash cannot be resolved or the
    /// session's evidence cannot be validated or replayed, and `ValueError` when the difference
    /// cannot be serialized.
    pub fn diff(
        &self,
        py: Python<'_>,
        from_hash: String,
        to_hash: String,
    ) -> PyResult<Model<SemanticDiff>> {
        let session_path = self.session_path.clone();

        py.detach(move || {
            let from = StatePoint::StateHash {
                state_hash: from_hash,
            };
            let to = StatePoint::StateHash {
                state_hash: to_hash,
            };
            let (diff, _, _) = AnalysisService::diff(&session_path, from, to).map_err(to_py_err)?;

            return Ok(Model(diff));
        })
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the output page cannot be read.
    #[pyo3(signature = (offset = None, limit = None))]
    pub fn read_output(
        &self,
        py: Python<'_>,
        offset: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<(Vec<u8>, u64, bool)> {
        let session_path = self.session_path.clone();
        let cursor = offset.map(|off| {
            return OutputCursor {
                session_path: session_path.clone(),
                byte_offset: off,
                event_seq: None,
            };
        });
        let limit_val = limit.unwrap_or(65536);

        py.detach(move || {
            let page =
                OutputService::read_page(&session_path, cursor, limit_val).map_err(to_py_err)?;

            return Ok((page.bytes, page.cursor.byte_offset, page.has_more));
        })
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range, and
    /// `TermtraceError` or a subclass when `pattern` is empty, the event log is invalid, the child
    /// exits or the session completes before the bytes appear, or the timeout elapses.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_output(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<bool> {
        let session_path = self.session_path.clone();
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;

        return py.detach(move || {
            let _outcome =
                OutputService::wait_for_bytes(&session_path, pattern.as_bytes(), timeout)
                    .map_err(to_py_err)?;

            return Ok(true);
        });
    }

    /// Read canonical event records as pretty JSON starting at an optional event sequence.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the event page cannot be read or serialized.
    #[pyo3(signature = (cursor = None, limit = None))]
    pub fn read_events(
        &self,
        py: Python<'_>,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<Vec<Model<Event>>> {
        let session_path = self.session_path.clone();
        let event_cursor = cursor.map(|seq| {
            return EventCursor {
                session_path: session_path.clone(),
                next_event_seq: seq,
            };
        });
        let limit_val = limit.unwrap_or(100);

        py.detach(move || {
            let page = EventsService::read_page(&session_path, event_cursor, limit_val)
                .map_err(to_py_err)?;

            return Ok(page.events.into_iter().map(Model).collect());
        })
    }

    /// Explain VT and semantic evidence around a target state point.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the target state cannot be resolved, the
    /// session's evidence cannot be validated or replayed, or the report cannot be serialized.
    #[pyo3(signature = (target_hash = None, context = None))]
    pub fn explain(
        &self,
        py: Python<'_>,
        target_hash: Option<String>,
        context: Option<u64>,
    ) -> PyResult<Model<ExplainReport>> {
        let session_path = self.session_path.clone();
        let point = target_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let ctx = context.unwrap_or(1);

        py.detach(move || {
            let report = AnalysisService::explain(&session_path, point, ctx).map_err(to_py_err)?;

            return Ok(Model(report));
        })
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when visual or canonical evidence fails validation or
    /// replay, or the report cannot be serialized.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn diagnose(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<Model<DiagnoseReport>> {
        let session_path = self.session_path.clone();
        let pct = broad_change_percent.unwrap_or(25);

        py.detach(move || {
            let report =
                AnalysisService::diagnose(&session_path, Vec::new(), pct).map_err(to_py_err)?;

            return Ok(Model(report));
        })
    }

    /// Perform full triage analysis on the most recent canonical action.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the most recent action cannot be analyzed from
    /// validated evidence or the report cannot be serialized.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn last_action(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<Model<LastActionAnalysis>> {
        let session_path = self.session_path.clone();
        let pct = broad_change_percent.unwrap_or(25);

        py.detach(move || {
            let analysis = AnalysisService::last_action(&session_path, pct).map_err(to_py_err)?;

            return Ok(Model(analysis));
        })
    }

    /// Build a bounded page of timeline entries as pretty JSON.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session's evidence cannot be read, validated,
    /// or replayed, or the page cannot be serialized.
    #[pyo3(signature = (offset = None, limit = None, include_hidden = None, include_controls = None))]
    pub fn timeline_page(
        &self,
        py: Python<'_>,
        offset: Option<u64>,
        limit: Option<u16>,
        include_hidden: Option<bool>,
        include_controls: Option<bool>,
    ) -> PyResult<Model<TimelinePage>> {
        let session_path = self.session_path.clone();
        let off = offset.unwrap_or(0);
        let lim = limit.unwrap_or(50);
        let hidden = include_hidden.unwrap_or(false);
        let controls = include_controls.unwrap_or(false);

        py.detach(move || {
            let page =
                AnalysisService::timeline_page(&session_path, hidden, controls, None, off, lim)
                    .map_err(to_py_err)?;

            return Ok(Model(page));
        })
    }
}
