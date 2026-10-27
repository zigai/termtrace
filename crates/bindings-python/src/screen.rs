//! Screen observation and state representations for Python.

use crate::errors::{query_err, to_py_err, unsatisfied_wait};
use crate::host::{Arg, Model};
use crate::locator::{PyLocator, PyLocatorSpec, PyQueryResult};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use pyo3_stub_gen::{PyStubType, TypeInfo};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use termtrace_model::terminal::{
    observation::{CaptureQuality, Observation},
    screen::{ScreenFrameContent, ScreenFrameSource, ScreenGeometry},
    state::{ColorState, CursorState, ModeState},
};
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults;
use termtrace_sdk::input::Reaction;
use termtrace_sdk::locator::{wait_duration_from_secs, LocatorSpec, ScreenFrameQuery, StyleFilter};
use termtrace_sdk::screen::{
    frame_text, FrameDetail, ScreenFrame, ScreenPredicate, ScreenProjectionRequest, ScreenRegion,
    ScreenWaitResult, StableCaptureOptions, StableScreenCapture,
};
use termtrace_sdk::StatePoint;

/// Immutable snapshot of a resolved presented terminal state.
#[gen_stub_pyclass]
#[pyclass(name = "ResolvedState", from_py_object)]
#[derive(Clone)]
pub struct PyResolvedState {
    /// Content-addressed semantic state hash.
    #[pyo3(get)]
    pub hash: String,
    /// Normalized plain text representation of the viewport.
    #[pyo3(get)]
    pub text: String,
    /// Zero-indexed cursor row, or 0 when the frame has no cursor row; see `cursor`.
    #[pyo3(get)]
    pub cursor_row: u16,
    /// Zero-indexed cursor column, or 0 when the frame has no cursor column; see `cursor`.
    #[pyo3(get)]
    pub cursor_col: u16,
    frame: Arc<ScreenFrame>,
}

impl PyResolvedState {
    /// Summarize one exact frame.
    pub fn from_frame(frame: &ScreenFrame) -> Self {
        return Self {
            hash: frame.state_hash.clone(),
            text: frame_text(frame),
            cursor_row: frame.cursor.row.unwrap_or_default(),
            cursor_col: frame.cursor.column.unwrap_or_default(),
            frame: Arc::new(frame.clone()),
        };
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyResolvedState {
    /// Complete cursor state; `row`, `column`, and `wide_tail` are `None` when unknown.
    #[getter]
    fn cursor(&self) -> Model<CursorState> {
        return Model(self.frame.cursor.clone());
    }

    /// The exact rich frame this state summarizes.
    #[getter]
    fn frame(&self) -> PyFrame {
        return PyFrame::new(Arc::clone(&self.frame));
    }

    fn __repr__(&self) -> String {
        return format!(
            "ResolvedState(hash='{}', cursor=({}, {}), text_len={})",
            self.hash,
            self.cursor_row,
            self.cursor_col,
            self.text.len()
        );
    }
}

/// Rich screen frame rendered against terminal geometry.
#[gen_stub_pyclass]
#[pyclass(name = "ScreenFrame", from_py_object)]
#[derive(Clone)]
pub struct PyFrame {
    /// Terminal viewport height in rows.
    #[pyo3(get)]
    pub rows: u16,
    /// Terminal viewport width in columns.
    #[pyo3(get)]
    pub cols: u16,
    /// Plain text string of all rendered rows.
    #[pyo3(get)]
    pub text: String,
    pub(crate) frame: Arc<ScreenFrame>,
}

impl PyFrame {
    /// Wrap one exact frame.
    pub fn new(frame: Arc<ScreenFrame>) -> Self {
        return Self {
            rows: frame.geometry.rows,
            cols: frame.geometry.cols,
            text: frame_text(&frame),
            frame,
        };
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyFrame {
    /// Projection format version.
    #[getter]
    fn version(&self) -> u32 {
        return self.frame.version;
    }

    /// Complete semantic state identity.
    #[getter]
    fn state_hash(&self) -> String {
        return self.frame.state_hash.clone();
    }

    /// Projection identity of this exact frame.
    #[getter]
    fn frame_hash(&self) -> String {
        return self.frame.frame_hash.clone();
    }

    /// Whether this state was presentation eligible.
    #[getter]
    fn presented(&self) -> bool {
        return self.frame.presented;
    }

    /// Settlement label: `live`, `stable`, `completed`, or `incomplete`.
    #[getter]
    fn capture_quality(&self) -> Model<CaptureQuality> {
        return Model(self.frame.capture_quality);
    }

    /// Exact source occurrence, tagged by `source`.
    #[getter]
    fn source(&self) -> Model<ScreenFrameSource> {
        return Model(self.frame.source.clone());
    }

    /// Full viewport geometry `{cols, rows}`.
    #[getter]
    fn geometry(&self) -> Model<ScreenGeometry> {
        return Model(self.frame.geometry);
    }

    /// Validated requested half-open region.
    #[getter]
    fn requested_region(&self) -> Model<ScreenRegion> {
        return Model(self.frame.requested_region);
    }

    /// Actually returned half-open region.
    #[getter]
    fn returned_region(&self) -> Model<ScreenRegion> {
        return Model(self.frame.returned_region);
    }

    /// Complete cursor state; `row`, `column`, and `wide_tail` are `None` when unknown.
    #[getter]
    fn cursor(&self) -> Model<CursorState> {
        return Model(self.frame.cursor.clone());
    }

    /// Terminal modes, including the complete ANSI and DEC mode inventory.
    #[getter]
    fn modes(&self) -> Model<ModeState> {
        return Model(self.frame.modes.clone());
    }

    /// Default colors, explicit cursor color, and the active 256-color palette.
    #[getter]
    fn colors(&self) -> Model<ColorState> {
        return Model(self.frame.colors.clone());
    }

    /// Structured rows tagged by `detail`: `none`, `text`, `spans`, or `cells`.
    #[getter]
    fn content(&self) -> Model<ScreenFrameContent> {
        return Model(self.frame.data.clone());
    }

    /// Evaluate a locator specification against this exact frame without capturing.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when the specification does not fit this frame.
    pub fn query(&self, spec: PyRef<'_, PyLocatorSpec>) -> PyResult<PyQueryResult> {
        return self
            .frame
            .query(spec.spec.clone())
            .map(PyQueryResult::new)
            .map_err(|error| return query_err(&error));
    }

    /// Canonical model JSON of this frame.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when the frame cannot be serialized.
    pub fn to_json(&self) -> PyResult<String> {
        return serde_json::to_string(self.frame.as_ref())
            .map_err(|error| return PyValueError::new_err(error.to_string()));
    }

    /// Decode canonical model JSON into a frame, rejecting unknown or missing fields.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `json` is not a valid screen frame.
    #[staticmethod]
    pub fn from_json(json: &str) -> PyResult<Self> {
        return serde_json::from_str::<ScreenFrame>(json)
            .map(|frame| return Self::new(Arc::new(frame)))
            .map_err(|error| return PyValueError::new_err(error.to_string()));
    }

    fn __repr__(&self) -> String {
        return format!(
            "ScreenFrame({}x{}, text_len={})",
            self.cols,
            self.rows,
            self.text.len()
        );
    }
}

fn projection(
    detail: Option<Arg<FrameDetail>>,
    region: Option<Arg<ScreenRegion>>,
) -> ScreenProjectionRequest {
    return ScreenProjectionRequest {
        detail: detail.map(|detail| return detail.0).unwrap_or_default(),
        region: region.map(|region| return region.0),
    };
}

fn seconds(value: Option<f64>, default: Duration) -> PyResult<Duration> {
    return value.map_or(Ok(default), |seconds| {
        return wait_duration_from_secs(seconds).map_err(PyValueError::new_err);
    });
}

fn reaction(object: &Bound<'_, PyAny>) -> PyResult<Reaction> {
    let dict = object
        .cast::<PyDict>()
        .map_err(|_wrong_type| return PyTypeError::new_err("reaction must be a dict"))?;

    let mut reaction = Reaction::new();
    for (name, value) in dict.iter() {
        let name = name
            .cast::<PyString>()
            .map_err(|_wrong_type| return PyTypeError::new_err("reaction keys must be str"))?
            .to_str()?
            .to_owned();
        match name.as_str() {
            "changed_from" => reaction.changed_from = value.extract::<Option<String>>()?,
            "timeout_secs" => {
                reaction.timeout = seconds(Some(value.extract::<f64>()?), reaction.timeout)?;
            }
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown reaction field `{other}`"
                )));
            }
        }
    }

    return Ok(reaction);
}

/// A model value returned as its generated type plus a `frame` key holding the exact frame.
pub struct Framed<T> {
    value: T,
    frame: ScreenFrame,
}

impl From<StableScreenCapture> for Framed<StableScreenCapture> {
    fn from(capture: StableScreenCapture) -> Self {
        let frame = capture.frame.clone();

        return Self {
            value: capture,
            frame,
        };
    }
}

impl From<termtrace_model::terminal::inspection::ScreenWaitOutcome> for Framed<ScreenWaitResult> {
    fn from(outcome: termtrace_model::terminal::inspection::ScreenWaitOutcome) -> Self {
        return Self {
            value: ScreenWaitResult::from(&outcome),
            frame: outcome.frame,
        };
    }
}

impl<'py, T: Serialize> IntoPyObject<'py> for Framed<T> {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        let object = crate::host::to_python(py, &self.value)?;
        object.set_item("frame", PyFrame::new(Arc::new(self.frame)))?;

        return Ok(object);
    }
}

impl<T: schemars::JsonSchema> PyStubType for Framed<T> {
    fn type_output() -> TypeInfo {
        return Model::<T>::type_output();
    }
}

/// Screen inspection capability attached to a live or completed session.
#[gen_stub_pyclass]
#[pyclass(name = "Screen", from_py_object)]
#[derive(Clone)]
pub struct PyScreen {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
}

impl PyScreen {
    fn latest_frame(&self) -> PyResult<ScreenFrame> {
        return self
            .terminal
            .attach(&self.session_path)
            .and_then(|session| return session.frame(ScreenProjectionRequest::default()))
            .map_err(to_py_err);
    }

    fn bind(&self, spec: LocatorSpec) -> PyLocator {
        return PyLocator::new(self.terminal.clone(), self.session_path.clone(), spec);
    }

    /// Wait for literal text on the presented screen; an unmet wait raises `TimeoutError`.
    pub(crate) fn blocking_wait_for_text(
        &self,
        pattern: &str,
        timeout: Duration,
    ) -> PyResult<PyResolvedState> {
        let session = self
            .terminal
            .attach(&self.session_path)
            .map_err(to_py_err)?;

        let predicate = ScreenPredicate::Contains {
            text: pattern.to_owned(),
        };
        let outcome = session
            .wait_screen(predicate, Duration::ZERO, timeout)
            .map_err(to_py_err)?;

        if !outcome.is_satisfied() {
            return Err(unsatisfied_wait(format!(
                "timed out waiting for pattern: '{pattern}'"
            )));
        }
        return Ok(PyResolvedState::from_frame(&outcome.frame));
    }

    /// Wait until the presented screen stays unchanged for the default idle interval.
    pub(crate) fn blocking_wait_for_idle(&self, timeout: Duration) -> PyResult<PyResolvedState> {
        let capture = self
            .terminal
            .attach(&self.session_path)
            .and_then(|session| {
                return session.capture_stable(StableCaptureOptions {
                    timeout,
                    ..StableCaptureOptions::default()
                });
            })
            .map_err(to_py_err)?;

        return Ok(PyResolvedState::from_frame(&capture.frame));
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyScreen {
    /// Return the latest presented screen frame with the requested detail (`none`, `text`,
    /// `spans` by default, or `cells`, which requires `region`) and optional half-open region.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `detail` or `region` is malformed, and `TermtraceError` or a
    /// subclass when the session cannot be attached or the frame cannot be projected.
    #[pyo3(signature = (detail = None, region = None))]
    pub fn frame(
        &self,
        py: Python<'_>,
        detail: Option<Arg<FrameDetail>>,
        region: Option<Arg<ScreenRegion>>,
    ) -> PyResult<PyFrame> {
        let projection = projection(detail, region);

        return py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.frame(projection))
                .map(|frame| return PyFrame::new(Arc::new(frame)))
                .map_err(to_py_err);
        });
    }

    /// Return a frame at any state point of a completed session, or the latest presented
    /// state of a live one.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
    /// a live session is asked for another point or hidden states, or resolution fails.
    #[pyo3(signature = (point, include_hidden = false, detail = None, region = None))]
    pub fn frame_at(
        &self,
        py: Python<'_>,
        point: Arg<StatePoint>,
        include_hidden: bool,
        detail: Option<Arg<FrameDetail>>,
        region: Option<Arg<ScreenRegion>>,
    ) -> PyResult<PyFrame> {
        let point = point.0;
        let projection = projection(detail, region);

        return py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.frame_at(point, include_hidden, projection))
                .map(|frame| return PyFrame::new(Arc::new(frame)))
                .map_err(to_py_err);
        });
    }

    /// Return the complete observation of a state point (latest presented by default).
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `point` is malformed, and `TermtraceError` or a subclass when a
    /// live session is asked for another point, or observation fails.
    #[pyo3(signature = (point = None, include_hidden = false))]
    pub fn observe(
        &self,
        py: Python<'_>,
        point: Option<Arg<StatePoint>>,
        include_hidden: bool,
    ) -> PyResult<Model<Observation>> {
        let point = point.map_or(StatePoint::LatestPresented, |point| return point.0);

        let observation = py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.observe(point, include_hidden))
                .map_err(to_py_err);
        })?;

        return Ok(Model(observation));
    }

    /// Capture one exact frame once it stays unchanged for `stable_for_secs`, optionally after a
    /// `reaction` (`{"changed_from": str | None, "timeout_secs": float}`).
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
    /// the capture fails or, unless `allow_incomplete`, does not settle.
    #[pyo3(signature = (
        stable_for_secs = None,
        timeout_secs = None,
        detail = None,
        region = None,
        allow_incomplete = false,
        reaction = None,
    ))]
    #[expect(
        clippy::too_many_arguments,
        reason = "each keyword argument maps to one stable-capture option"
    )]
    pub fn capture_stable(
        &self,
        py: Python<'_>,
        stable_for_secs: Option<f64>,
        timeout_secs: Option<f64>,
        detail: Option<Arg<FrameDetail>>,
        region: Option<Arg<ScreenRegion>>,
        allow_incomplete: bool,
        reaction: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Framed<StableScreenCapture>> {
        let options = StableCaptureOptions {
            stable_for: seconds(stable_for_secs, defaults::IDLE_STABLE_FOR)?,
            timeout: seconds(timeout_secs, defaults::WAIT_TIMEOUT)?,
            projection: projection(detail, region),
            allow_incomplete,
            reaction: reaction.map(self::reaction).transpose()?,
        };

        let capture = py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.capture_stable(options))
                .map_err(to_py_err);
        })?;

        return Ok(Framed::from(capture));
    }

    /// Wait on the actor for a screen predicate and return the outcome without raising when it
    /// is unmatched; `quality` tells a deadline from child completion or output closure.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when an argument is malformed, and `TermtraceError` or a subclass when
    /// the actor rejects the predicate or the wait fails.
    #[pyo3(signature = (predicate, stable_for_secs = None, timeout_secs = None))]
    pub fn wait_for(
        &self,
        py: Python<'_>,
        predicate: Arg<ScreenPredicate>,
        stable_for_secs: Option<f64>,
        timeout_secs: Option<f64>,
    ) -> PyResult<Framed<ScreenWaitResult>> {
        let predicate = predicate.0;
        let stable_for = seconds(stable_for_secs, Duration::ZERO)?;
        let timeout = seconds(timeout_secs, defaults::WAIT_TIMEOUT)?;

        let outcome = py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.wait_screen(predicate, stable_for, timeout))
                .map_err(to_py_err);
        })?;

        return Ok(Framed::from(outcome));
    }

    /// Bind a locator specification to this screen without capturing; the default matches
    /// every cell.
    #[pyo3(signature = (spec = None))]
    pub fn locator(&self, spec: Option<PyRef<'_, PyLocatorSpec>>) -> PyLocator {
        return self.bind(
            spec.map(|spec| return spec.spec.clone())
                .unwrap_or_default(),
        );
    }

    /// Bind a nonempty literal text locator; `exact` compares the complete row or region text.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `text` is empty.
    #[pyo3(signature = (text, exact = false))]
    pub fn get_by_text(&self, text: String, exact: bool) -> PyResult<PyLocator> {
        let spec = if exact { LocatorSpec::exact(text) } else { LocatorSpec::text(text) };

        return spec
            .map(|spec| return self.bind(spec))
            .map_err(|error| return query_err(&error));
    }

    /// Bind a Rust regex syntax locator.
    ///
    /// # Errors
    ///
    /// Raises `TypeError` for anything but a `str` (including `re.Pattern`) and `QueryError`
    /// when the pattern is invalid or can match zero-width text.
    pub fn get_by_regex(&self, pattern: &Bound<'_, PyAny>) -> PyResult<PyLocator> {
        return crate::locator::regex_spec(pattern).map(|spec| return self.bind(spec));
    }

    /// Bind one absolute row.
    pub fn row(&self, row: u16) -> PyLocator {
        return self.bind(LocatorSpec::default().in_row(row));
    }

    /// Bind one nonempty absolute half-open region.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `region` is malformed and `QueryError` when it is empty.
    pub fn region(&self, region: Arg<ScreenRegion>) -> PyResult<PyLocator> {
        return LocatorSpec::default()
            .in_region(region.0)
            .map(|spec| return self.bind(spec))
            .map_err(|error| return query_err(&error));
    }

    /// Bind a cell appearance filter.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `style` is malformed.
    pub fn filter(&self, style: Arg<StyleFilter>) -> PyLocator {
        return self.bind(LocatorSpec::default().with_style(style.0));
    }

    /// Return the normalized plain text of the latest presented frame.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass when the session cannot be attached or the frame
    /// cannot be captured.
    pub fn text(&self, py: Python<'_>) -> PyResult<String> {
        return py.detach(|| return Ok(frame_text(&self.latest_frame()?)));
    }

    /// Wait until the presented screen contains the specified text pattern.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
    /// `TimeoutError` when the text is not presented in time, and `TermtraceError` or a subclass
    /// when the session cannot be attached or the wait fails.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn wait_for_text(
        &self,
        py: Python<'_>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;

        return py.detach(|| return self.blocking_wait_for_text(&pattern, timeout));
    }

    /// Wait until the presented screen remains unchanged for an idle window.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range,
    /// `TimeoutError` when the screen does not settle in time, and `TermtraceError` or a subclass
    /// when the session cannot be attached or the capture fails.
    #[pyo3(signature = (timeout_secs = None))]
    pub fn wait_for_idle(
        &self,
        py: Python<'_>,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyResolvedState> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;

        return py.detach(|| return self.blocking_wait_for_idle(timeout));
    }
}
