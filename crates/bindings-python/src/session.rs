//! Logical session handle and forensic exports for Python.

use crate::actions::PyActionBuilder;
use crate::errors::to_py_err;
use crate::screen::PyScreen;
use pyo3::prelude::*;
use std::path::PathBuf;
use termtrace_model::artifact::bundle::BundleProfile;
use termtrace_model::artifact::save::SaveFormat;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::terminal::state::MouseButton;
use termtrace_sdk::actions::ActionOptions;
use termtrace_sdk::analysis::AnalysisService;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::events::{EventCursor, EventsService};
use termtrace_sdk::output::{OutputCursor, OutputService};

/// A logical Termtrace session reference.
#[pyclass(name = "Session", from_py_object)]
#[derive(Clone)]
pub struct PySession {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
    pub(crate) session_id: String,
}

#[pymethods]
impl PySession {
    /// Canonical session identity string.
    #[getter]
    pub fn session_id(&self) -> &str {
        return &self.session_id;
    }

    /// Filesystem path to the session directory.
    #[getter]
    pub fn path(&self) -> &str {
        return &self.session_path;
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
        return PyActionBuilder {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
            atoms: Vec::new(),
            options: ActionOptions::default(),
        };
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
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let btn = match button
            .as_deref()
            .unwrap_or("left")
            .to_ascii_lowercase()
            .as_str()
        {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            "wheel_up" | "wheelup" => MouseButton::WheelUp,
            "wheel_down" | "wheeldown" => MouseButton::WheelDown,
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown mouse button: '{other}'"
                )))
            }
        };
        let is_strict = strict.unwrap_or(false);

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            session
                .pointer()
                .click(column, row, btn, is_strict)
                .map_err(to_py_err)?;

            return Ok(());
        })
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
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let line_count = lines.unwrap_or(1);
        let is_strict = strict.unwrap_or(false);

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            session
                .pointer()
                .scroll(column, row, line_count, is_strict)
                .map_err(to_py_err)?;

            return Ok(());
        })
    }

    /// Report terminal focus state.
    #[pyo3(signature = (focused, strict = None))]
    pub fn pointer_focus(
        &self,
        py: Python<'_>,
        focused: bool,
        strict: Option<bool>,
    ) -> PyResult<()> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let is_strict = strict.unwrap_or(false);

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            session
                .pointer()
                .focus(focused, is_strict)
                .map_err(to_py_err)?;

            return Ok(());
        })
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
        let session_path = PathBuf::from(&self.session_path);
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

            return serde_json::to_string_pretty(&outcome.manifest).map_err(|e| {
                to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: dest,
                    json_path: String::new(),
                    source: e,
                })
            });
        })
    }

    /// Export session terminal playback as an asciicast v2 file.
    pub fn export_asciicast(&self, py: Python<'_>, output_path: String) -> PyResult<()> {
        let session_path = PathBuf::from(&self.session_path);
        let target = PathBuf::from(output_path);

        py.detach(move || {
            ArtifactService::export_asciicast(&session_path, &target).map_err(to_py_err)?;

            return Ok(());
        })
    }

    /// Create a compressed evidence bundle (.tar.zst) for archiving or forensics.
    pub fn create_bundle(&self, py: Python<'_>, output_path: String) -> PyResult<()> {
        let session_path = PathBuf::from(&self.session_path);
        let target = PathBuf::from(output_path);

        py.detach(move || {
            ArtifactService::create_bundle(&session_path, &target, BundleProfile::Forensic)
                .map_err(to_py_err)?;

            return Ok(());
        })
    }

    /// Compute semantic difference between two presented states and return JSON.
    pub fn diff(&self, py: Python<'_>, from_hash: String, to_hash: String) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);

        py.detach(move || {
            let from = StatePoint::StateHash {
                state_hash: from_hash,
            };
            let to = StatePoint::StateHash {
                state_hash: to_hash,
            };
            let (diff, _, _) = AnalysisService::diff(&session_path, from, to).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&diff)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()));
        })
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    #[pyo3(signature = (offset = None, limit = None))]
    pub fn read_output(
        &self,
        py: Python<'_>,
        offset: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<(Vec<u8>, u64, bool)> {
        let session_path = PathBuf::from(&self.session_path);
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
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_output(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<bool> {
        let session_path = PathBuf::from(&self.session_path);
        let timeout = crate::parse_timeout(timeout_secs, 10.0)?;

        return py.detach(move || {
            let _outcome =
                OutputService::wait_for_bytes(&session_path, pattern.as_bytes(), timeout)
                    .map_err(to_py_err)?;

            return Ok(true);
        });
    }

    /// Read canonical event records as pretty JSON starting at an optional event sequence.
    #[pyo3(signature = (cursor = None, limit = None))]
    pub fn read_events(
        &self,
        py: Python<'_>,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);
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

            return serde_json::to_string_pretty(&page.events).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: session_path,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Explain VT and semantic evidence around a target state point.
    #[pyo3(signature = (target_hash = None, context = None))]
    pub fn explain(
        &self,
        py: Python<'_>,
        target_hash: Option<String>,
        context: Option<u64>,
    ) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);
        let point = target_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let ctx = context.unwrap_or(1);

        py.detach(move || {
            let report = AnalysisService::explain(&session_path, point, ctx).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: session_path,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn diagnose(&self, py: Python<'_>, broad_change_percent: Option<u8>) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);
        let pct = broad_change_percent.unwrap_or(25);

        py.detach(move || {
            let report =
                AnalysisService::diagnose(&session_path, Vec::new(), pct).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&report).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: session_path,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Perform full triage analysis on the most recent canonical action.
    #[pyo3(signature = (broad_change_percent = None))]
    pub fn last_action(
        &self,
        py: Python<'_>,
        broad_change_percent: Option<u8>,
    ) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);
        let pct = broad_change_percent.unwrap_or(25);

        py.detach(move || {
            let analysis = AnalysisService::last_action(&session_path, pct).map_err(to_py_err)?;

            return serde_json::to_string_pretty(&analysis).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: session_path,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }

    /// Build a bounded page of timeline entries as pretty JSON.
    #[pyo3(signature = (offset = None, limit = None, include_hidden = None, include_controls = None))]
    pub fn timeline_page(
        &self,
        py: Python<'_>,
        offset: Option<u64>,
        limit: Option<u16>,
        include_hidden: Option<bool>,
        include_controls: Option<bool>,
    ) -> PyResult<String> {
        let session_path = PathBuf::from(&self.session_path);
        let off = offset.unwrap_or(0);
        let lim = limit.unwrap_or(50);
        let hidden = include_hidden.unwrap_or(false);
        let controls = include_controls.unwrap_or(false);

        py.detach(move || {
            let page =
                AnalysisService::timeline_page(&session_path, hidden, controls, None, off, lim)
                    .map_err(to_py_err)?;

            return serde_json::to_string_pretty(&page).map_err(|e| {
                return to_py_err(termtrace_sdk::Error::InvalidJson {
                    path: session_path,
                    json_path: String::new(),
                    source: e,
                });
            });
        })
    }
}
