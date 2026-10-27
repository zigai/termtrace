//! Locator specifications, session-bound locators, query results, and cancellable waits.
//!
//! Every query runs in the shared Rust evaluator; Python never matches text itself.

use crate::errors::{locator_err, query_err};
use crate::host::{Arg, Model};
use crate::screen::PyFrame;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyString;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::defaults;
use termtrace_sdk::locator::{
    wait_duration_from_secs, Locator, LocatorCondition, LocatorSpec, LocatorWaitOptions,
    LocatorWaitResult, MatchRange, MatchedCell, QueryDescription, QueryResult, ScreenMatch,
    StyleFilter, WaitCancellation,
};
use termtrace_sdk::screen::ScreenRegion;

/// Build a regex specification from a Rust regex syntax string.
pub(crate) fn regex_spec(pattern: &Bound<'_, PyAny>) -> PyResult<LocatorSpec> {
    let source = pattern.cast::<PyString>().map_err(|_wrong_type| {
        return PyTypeError::new_err(
            "expected a Rust regex syntax str; re.Pattern and other objects are not accepted \
             (put flags inline, e.g. \"(?i)save\")",
        );
    })?;

    return LocatorSpec::native_regex(source.to_str()?).map_err(|error| return query_err(&error));
}

fn seconds(value: Option<f64>, default: Duration) -> PyResult<Duration> {
    return value.map_or(Ok(default), |seconds| {
        return wait_duration_from_secs(seconds).map_err(PyValueError::new_err);
    });
}

/// Cooperative cancellation for locator waits, shared across threads.
///
/// Cancelling wakes a sleeping wait at once; a capture already sent to the session still runs to
/// its bounded end before the wait returns.
#[gen_stub_pyclass]
#[pyclass(name = "CancelToken", frozen, from_py_object)]
#[derive(Clone, Default)]
pub struct PyCancelToken {
    pub(crate) inner: WaitCancellation,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyCancelToken {
    /// Create an uncancelled token.
    #[new]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Request cancellation; idempotent and safe from any thread.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Whether cancellation was requested.
    #[getter]
    pub fn cancelled(&self) -> bool {
        return self.inner.is_cancelled();
    }
}

/// Immutable locator specification evaluated by the shared Rust engine.
#[gen_stub_pyclass]
#[pyclass(name = "LocatorSpec", frozen, from_py_object)]
#[derive(Clone, Default)]
pub struct PyLocatorSpec {
    pub(crate) spec: LocatorSpec,
}

impl PyLocatorSpec {
    fn wrap(spec: Result<LocatorSpec, termtrace_sdk::locator::QueryError>) -> PyResult<Self> {
        return spec
            .map(|spec| return Self { spec })
            .map_err(|error| return query_err(&error));
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyLocatorSpec {
    /// A specification matching every cell; narrow it with the builder methods.
    #[new]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Match every occurrence of nonempty literal text, overlapping occurrences included.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `text` is empty.
    #[staticmethod]
    pub fn text(text: String) -> PyResult<Self> {
        return Self::wrap(LocatorSpec::text(text));
    }

    /// Match the complete selected row or region text.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `text` is empty.
    #[staticmethod]
    pub fn exact(text: String) -> PyResult<Self> {
        return Self::wrap(LocatorSpec::exact(text));
    }

    /// Match non-overlapping occurrences of a Rust regex syntax pattern.
    ///
    /// # Errors
    ///
    /// Raises `TypeError` for anything but a `str` and `QueryError` when the pattern is invalid
    /// or can match zero-width text.
    #[staticmethod]
    pub fn regex(pattern: &Bound<'_, PyAny>) -> PyResult<Self> {
        return regex_spec(pattern).map(|spec| return Self { spec });
    }

    /// Restrict to one absolute row.
    pub fn in_row(&self, row: u16) -> Self {
        return Self {
            spec: self.spec.clone().in_row(row),
        };
    }

    /// Restrict to one nonempty absolute half-open region.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `region` is malformed and `QueryError` when it is empty.
    pub fn in_region(&self, region: Arg<ScreenRegion>) -> PyResult<Self> {
        return Self::wrap(self.spec.clone().in_region(region.0));
    }

    /// Require every covered cell to satisfy a style filter.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `style` is malformed.
    pub fn with_style(&self, style: Arg<StyleFilter>) -> Self {
        return Self {
            spec: self.spec.clone().with_style(style.0),
        };
    }

    /// Let text matches continue across native soft-wrapped rows.
    #[pyo3(signature = (enabled = true))]
    pub fn across_soft_wraps(&self, enabled: bool) -> Self {
        return Self {
            spec: self.spec.clone().across_soft_wraps(enabled),
        };
    }

    /// Combine with another specification holding no second text matcher or style filter.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when the scopes conflict or both hold a text matcher or style filter.
    pub fn and_(&self, other: PyRef<'_, Self>) -> PyResult<Self> {
        return Self::wrap(self.spec.clone().and(other.spec.clone()));
    }

    fn __repr__(&self) -> String {
        return format!("LocatorSpec({:?})", self.spec);
    }
}

/// One match tied to the exact frame that produced it.
#[gen_stub_pyclass]
#[pyclass(name = "ScreenMatch", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyScreenMatch {
    pub(crate) inner: ScreenMatch,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyScreenMatch {
    /// The exact frame evaluated.
    #[getter]
    fn frame(&self) -> PyFrame {
        return PyFrame::new(Arc::clone(&self.inner.frame));
    }

    /// Ordered half-open `{row, start_column, end_column}` cell ranges.
    #[getter]
    fn ranges(&self) -> Vec<Model<MatchRange>> {
        return self.inner.ranges.iter().copied().map(Model).collect();
    }

    /// Matched text with spaces and graphemes preserved.
    #[getter]
    fn text(&self) -> String {
        return self.inner.text.to_string();
    }

    /// Covered cells `{row, column, width, grapheme, wide_kind, appearance}`, at most 128.
    #[getter]
    fn cells(&self) -> Vec<Model<MatchedCell>> {
        return self
            .inner
            .covered_cells
            .iter()
            .cloned()
            .map(Model)
            .collect();
    }

    /// Covered cells omitted from `cells`.
    #[getter]
    fn omitted_cell_count(&self) -> usize {
        return self.inner.omitted_cell_count;
    }

    /// Bounded normalized query `{text, truncated}`.
    #[getter]
    fn query(&self) -> Model<QueryDescription> {
        return Model(self.inner.query.clone());
    }

    fn __repr__(&self) -> String {
        return format!(
            "ScreenMatch(text={:?}, ranges={:?})",
            self.inner.text, self.inner.ranges
        );
    }
}

fn matches(matches: &[ScreenMatch]) -> Vec<PyScreenMatch> {
    return matches
        .iter()
        .map(|screen_match| {
            return PyScreenMatch {
                inner: screen_match.clone(),
            };
        })
        .collect();
}

/// Matches evaluated over one exact frame.
#[gen_stub_pyclass]
#[pyclass(name = "QueryResult", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyQueryResult {
    pub(crate) inner: QueryResult,
}

impl PyQueryResult {
    /// Wrap one evaluation.
    pub fn new(inner: QueryResult) -> Self {
        return Self { inner };
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryResult {
    /// The exact frame evaluated.
    #[getter]
    fn frame(&self) -> PyFrame {
        return PyFrame::new(Arc::clone(self.inner.frame()));
    }

    /// Every match in row and column order.
    fn all(&self) -> Vec<PyScreenMatch> {
        return matches(self.inner.all());
    }

    /// Exact match count.
    fn count(&self) -> usize {
        return self.inner.count();
    }

    /// Require exactly one match.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` with kind `no_match` or `multiple_matches`.
    fn single(&self) -> PyResult<PyScreenMatch> {
        return self
            .inner
            .single()
            .map(|inner| {
                return PyScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_err(&error));
    }

    /// Select the first match, rejecting absence but not ambiguity.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` with kind `no_match`.
    fn first(&self) -> PyResult<PyScreenMatch> {
        return self
            .inner
            .first()
            .map(|inner| {
                return PyScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_err(&error));
    }

    /// Select one zero-based match.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` with kind `index_out_of_bounds`.
    fn nth(&self, index: usize) -> PyResult<PyScreenMatch> {
        return self
            .inner
            .nth(index)
            .map(|inner| {
                return PyScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_err(&error));
    }
}

/// A satisfied locator wait: the satisfying frame and its matches.
#[gen_stub_pyclass]
#[pyclass(name = "LocatorWaitResult", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyLocatorWaitResult {
    inner: LocatorWaitResult,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyLocatorWaitResult {
    /// The exact satisfying frame.
    #[getter]
    fn frame(&self) -> PyFrame {
        return PyFrame::new(Arc::clone(&self.inner.frame));
    }

    /// Matches from that frame; empty for an `absent` condition.
    #[getter]
    fn matches(&self) -> Vec<PyScreenMatch> {
        return matches(&self.inner.matches);
    }
}

/// Lazy locator bound to a session; every operation captures one fresh frame.
#[gen_stub_pyclass]
#[pyclass(name = "Locator", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyLocator {
    terminal: Terminal,
    session_path: PathBuf,
    spec: LocatorSpec,
}

impl PyLocator {
    /// Bind a specification to a session path.
    pub fn new(terminal: Terminal, session_path: PathBuf, spec: LocatorSpec) -> Self {
        return Self {
            terminal,
            session_path,
            spec,
        };
    }

    fn rebind(&self, spec: LocatorSpec) -> Self {
        return Self::new(self.terminal.clone(), self.session_path.clone(), spec);
    }

    fn run<T: Send>(
        &self,
        py: Python<'_>,
        operation: impl FnOnce(&Locator<'_>) -> Result<T, termtrace_sdk::locator::LocatorError> + Send,
    ) -> PyResult<T> {
        return py.detach(|| {
            let session: Session = self
                .terminal
                .attach(&self.session_path)
                .map_err(crate::errors::to_py_err)?;

            return operation(&session.locator(self.spec.clone())).map_err(locator_err);
        });
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyLocator {
    /// The bound specification.
    #[getter]
    fn spec(&self) -> PyLocatorSpec {
        return PyLocatorSpec {
            spec: self.spec.clone(),
        };
    }

    /// Combine with another specification without capturing.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when the scopes conflict or both hold a text matcher or style filter.
    fn and_(&self, other: PyRef<'_, PyLocatorSpec>) -> PyResult<Self> {
        return self
            .spec
            .clone()
            .and(other.spec.clone())
            .map(|spec| return self.rebind(spec))
            .map_err(|error| return query_err(&error));
    }

    /// Restrict to one absolute row without capturing.
    fn in_row(&self, row: u16) -> Self {
        return self.rebind(self.spec.clone().in_row(row));
    }

    /// Restrict to one nonempty absolute half-open region without capturing.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `region` is malformed and `QueryError` when it is empty.
    fn in_region(&self, region: Arg<ScreenRegion>) -> PyResult<Self> {
        return self
            .spec
            .clone()
            .in_region(region.0)
            .map(|spec| return self.rebind(spec))
            .map_err(|error| return query_err(&error));
    }

    /// Require a style filter without capturing.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when `style` is malformed.
    fn with_style(&self, style: Arg<StyleFilter>) -> Self {
        return self.rebind(self.spec.clone().with_style(style.0));
    }

    /// Let text matches continue across native soft-wrapped rows.
    #[pyo3(signature = (enabled = true))]
    fn across_soft_wraps(&self, enabled: bool) -> Self {
        return self.rebind(self.spec.clone().across_soft_wraps(enabled));
    }

    /// Capture one frame and evaluate it once.
    ///
    /// # Errors
    ///
    /// Raises `QueryError` when the specification does not fit the frame and `TermtraceError`
    /// or a subclass when the capture fails.
    fn evaluate(&self, py: Python<'_>) -> PyResult<PyQueryResult> {
        return self
            .run(py, |locator| return locator.evaluate())
            .map(PyQueryResult::new);
    }

    /// Capture once and return every match.
    ///
    /// # Errors
    ///
    /// Raises the errors of `evaluate`.
    fn all(&self, py: Python<'_>) -> PyResult<Vec<PyScreenMatch>> {
        return self
            .run(py, |locator| return locator.all())
            .map(|found| return matches(&found));
    }

    /// Capture once and count matches.
    ///
    /// # Errors
    ///
    /// Raises the errors of `evaluate`.
    fn count(&self, py: Python<'_>) -> PyResult<usize> {
        return self.run(py, |locator| return locator.count());
    }

    /// Capture once and require exactly one match.
    ///
    /// # Errors
    ///
    /// Raises the errors of `evaluate`, and `QueryError` with kind `no_match` or
    /// `multiple_matches`.
    fn single(&self, py: Python<'_>) -> PyResult<PyScreenMatch> {
        return self
            .run(py, |locator| return locator.single())
            .map(|inner| return PyScreenMatch { inner });
    }

    /// Capture once and select the first match.
    ///
    /// # Errors
    ///
    /// Raises the errors of `evaluate`, and `QueryError` with kind `no_match`.
    fn first(&self, py: Python<'_>) -> PyResult<PyScreenMatch> {
        return self
            .run(py, |locator| return locator.first())
            .map(|inner| return PyScreenMatch { inner });
    }

    /// Capture once and select one zero-based match.
    ///
    /// # Errors
    ///
    /// Raises the errors of `evaluate`, and `QueryError` with kind `index_out_of_bounds`.
    fn nth(&self, py: Python<'_>, index: usize) -> PyResult<PyScreenMatch> {
        return self
            .run(py, |locator| return locator.nth(index))
            .map(|inner| return PyScreenMatch { inner });
    }

    /// Wait until at least one match exists; equivalent to `wait_for("exists", ...)` without
    /// cancellation.
    ///
    /// # Errors
    ///
    /// Raises the errors of `wait_for`.
    #[pyo3(signature = (timeout_secs = None, interval_secs = None))]
    fn wait(
        &self,
        py: Python<'_>,
        timeout_secs: Option<f64>,
        interval_secs: Option<f64>,
    ) -> PyResult<PyLocatorWaitResult> {
        let timeout = seconds(timeout_secs, defaults::WAIT_TIMEOUT)?;
        let interval = seconds(interval_secs, defaults::LOCATOR_POLL_INTERVAL)?;

        return self
            .run(py, |locator| return locator.wait(timeout, interval))
            .map(|inner| return PyLocatorWaitResult { inner });
    }

    /// Wait until at least one match holds in one unchanged state for `stable_for_secs`.
    ///
    /// # Errors
    ///
    /// Raises the errors of `wait_for`.
    #[pyo3(signature = (timeout_secs = None, interval_secs = None, stable_for_secs = None))]
    fn wait_stable(
        &self,
        py: Python<'_>,
        timeout_secs: Option<f64>,
        interval_secs: Option<f64>,
        stable_for_secs: Option<f64>,
    ) -> PyResult<PyLocatorWaitResult> {
        let timeout = seconds(timeout_secs, defaults::WAIT_TIMEOUT)?;
        let interval = seconds(interval_secs, defaults::LOCATOR_POLL_INTERVAL)?;
        let stable_for = seconds(stable_for_secs, Duration::ZERO)?;

        return self
            .run(py, |locator| {
                return locator.wait_stable(timeout, interval, stable_for);
            })
            .map(|inner| return PyLocatorWaitResult { inner });
    }

    /// Poll until `condition` (`"exists"`, `"absent"`, or `{"count": n}`) holds in one
    /// unchanged state for `stable_for_secs`.
    ///
    /// Another thread may call `cancel.cancel()`. A responsive session settles within about one
    /// 250 ms capture quantum plus a round trip; a session busy with other work can delay the
    /// return by up to about 5.25 s. No input is ever sent.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for malformed durations, `QueryError` for an invalid condition, zero
    /// timeout or interval, or `stable_for_secs` above `timeout_secs`, `TimeoutError` (kind
    /// `locator_timeout`, with the last `frame`) when unsatisfied in time, `CancelledError`
    /// (kind `cancelled`, with the last `frame`) after cancellation, and `TermtraceError` or a
    /// subclass when a capture fails.
    #[pyo3(signature = (
        condition = None,
        timeout_secs = None,
        interval_secs = None,
        stable_for_secs = None,
        cancel = None,
    ))]
    fn wait_for(
        &self,
        py: Python<'_>,
        condition: Option<Arg<LocatorCondition>>,
        timeout_secs: Option<f64>,
        interval_secs: Option<f64>,
        stable_for_secs: Option<f64>,
        cancel: Option<PyRef<'_, PyCancelToken>>,
    ) -> PyResult<PyLocatorWaitResult> {
        let condition = condition.map_or(LocatorCondition::Exists, |condition| return condition.0);
        let options = LocatorWaitOptions {
            condition,
            timeout: seconds(timeout_secs, defaults::WAIT_TIMEOUT)?,
            interval: seconds(interval_secs, defaults::LOCATOR_POLL_INTERVAL)?,
            stable_for: seconds(stable_for_secs, Duration::ZERO)?,
            cancel: cancel.map(|cancel| return cancel.inner.clone()),
        };

        return self
            .run(py, |locator| return locator.wait_for(options))
            .map(|inner| return PyLocatorWaitResult { inner });
    }

    fn __repr__(&self) -> String {
        return format!("Locator({:?})", self.spec);
    }
}
