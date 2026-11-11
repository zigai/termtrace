//! Locator specifications, session-bound locators, query results, and cancellable waits.
//!
//! Every query runs in the shared Rust evaluator; JavaScript never matches text itself.

use crate::host::{index, millis, Arg, Model};
use crate::{blocking_failure, to_napi_err, typed_error_with, JsFrame};
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use termtrace_model::protocol::mutation::MutationEffect;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::defaults;
use termtrace_sdk::locator::{
    Locator, LocatorCondition, LocatorError, LocatorSpec, LocatorWaitOptions, LocatorWaitResult,
    QueryError, QueryResult, ScreenMatch, StyleFilter, WaitCancellation,
};
use termtrace_sdk::screen::{ScreenFrame, ScreenRegion};

/// Translate a pure query failure into a typed error carrying its details.
pub(crate) fn query_error(error: &QueryError) -> napi::Error {
    return detailed(error.to_string(), error.clone(), LastFrame::NotCarried);
}

/// Translate a locator failure; timeouts and cancellations carry the last frame.
pub(crate) fn locator_error(error: LocatorError) -> napi::Error {
    let message = error.to_string();
    let details = error.failure();

    return match error {
        LocatorError::Capture(capture) => to_napi_err(capture),
        LocatorError::Timeout { frame, .. } | LocatorError::Cancelled { frame, .. } => {
            detailed(message, details, LastFrame::Carried(frame))
        }
        LocatorError::Query(_) => detailed(message, details, LastFrame::NotCarried),
    };
}

enum LastFrame {
    NotCarried,
    Carried(Option<Arc<ScreenFrame>>),
}

fn detailed<T: serde::Serialize>(message: String, details: T, frame: LastFrame) -> napi::Error {
    #[derive(serde::Serialize)]
    struct Failure<T> {
        details: T,
        #[serde(flatten)]
        frame: ErrorFrame,
    }

    let kind = match serde_json::to_value(&details) {
        Ok(value) => value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("invalid_query")
            .to_owned(),
        Err(error) => return napi::Error::from_reason(error.to_string()),
    };

    #[derive(serde::Serialize)]
    #[serde(untagged)]
    enum ErrorFrame {
        NotCarried {},
        Carried { frame_json: Option<String> },
    }

    let frame = match frame {
        LastFrame::NotCarried => ErrorFrame::NotCarried {},
        LastFrame::Carried(frame) => ErrorFrame::Carried {
            frame_json: frame.and_then(|frame| serde_json::to_string(frame.as_ref()).ok()),
        },
    };

    return typed_error_with(
        message,
        &kind,
        &kind,
        MutationEffect::NotApplied,
        Failure { details, frame },
    );
}

/// Build a regex specification from a Rust regex syntax string.
pub(crate) fn regex_spec(pattern: Arg<String>) -> napi::Result<LocatorSpec> {
    let source = pattern.0;
    return LocatorSpec::native_regex(source).map_err(|error| return query_error(&error));
}

/// Cooperative cancellation for locator waits.
///
/// Cancelling wakes a sleeping wait at once; a capture already sent to the session still runs to
/// its bounded end before the wait's promise settles.
#[napi(js_name = "CancelToken")]
#[derive(Clone, Default)]
pub struct JsCancelToken {
    pub(crate) inner: WaitCancellation,
}

#[napi]
impl JsCancelToken {
    /// Create an uncancelled token.
    #[napi(constructor)]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Request cancellation; idempotent.
    #[napi]
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Whether cancellation was requested.
    #[napi(getter)]
    pub fn cancelled(&self) -> bool {
        return self.inner.is_cancelled();
    }
}

/// Immutable locator specification evaluated by the shared Rust engine.
#[napi(js_name = "LocatorSpec")]
#[derive(Clone, Default)]
pub struct JsLocatorSpec {
    pub(crate) spec: LocatorSpec,
}

impl JsLocatorSpec {
    fn wrap(spec: Result<LocatorSpec, QueryError>) -> napi::Result<Self> {
        return spec
            .map(|spec| return Self { spec })
            .map_err(|error| return query_error(&error));
    }
}

#[napi]
impl JsLocatorSpec {
    /// A specification matching every cell; narrow it with the builder methods.
    #[napi(constructor)]
    pub fn new() -> Self {
        return Self::default();
    }

    /// Match every occurrence of nonempty literal text, overlapping occurrences included.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `text` is empty.
    #[napi(factory)]
    pub fn text(text: String) -> napi::Result<Self> {
        return Self::wrap(LocatorSpec::text(text));
    }

    /// Match the complete selected row or region text.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `text` is empty.
    #[napi(factory)]
    pub fn exact(text: String) -> napi::Result<Self> {
        return Self::wrap(LocatorSpec::exact(text));
    }

    /// Match non-overlapping occurrences of a Rust regex syntax pattern string.
    ///
    /// # Errors
    ///
    /// Fails for anything but a string (including `RegExp`), and with kind `invalid_query` when
    /// the pattern is invalid or can match zero-width text.
    #[napi(factory)]
    pub fn regex(pattern: Arg<String>) -> napi::Result<Self> {
        return regex_spec(pattern).map(|spec| return Self { spec });
    }

    /// Restrict to one absolute row.
    ///
    /// # Errors
    ///
    /// Fails when `row` is not a whole number from 0 through 65535.
    #[napi]
    pub fn in_row(&self, row: f64) -> napi::Result<Self> {
        let row = crate::host::cell_coordinate(row, "row")?;

        return Ok(Self {
            spec: self.spec.clone().in_row(row),
        });
    }

    /// Restrict to one nonempty absolute half-open region.
    ///
    /// # Errors
    ///
    /// Fails when `region` is malformed, and with kind `invalid_query` when it is empty.
    #[napi]
    pub fn in_region(&self, region: Arg<ScreenRegion>) -> napi::Result<Self> {
        let region = region.0;

        return Self::wrap(self.spec.clone().in_region(region));
    }

    /// Require every covered cell to satisfy a style filter.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `style` is malformed.
    #[napi]
    pub fn with_style(&self, style: Arg<StyleFilter>) -> Self {
        return Self {
            spec: self.spec.clone().with_style(style.0),
        };
    }

    /// Let text matches continue across native soft-wrapped rows.
    #[napi]
    pub fn across_soft_wraps(&self, enabled: Option<bool>) -> Self {
        return Self {
            spec: self.spec.clone().across_soft_wraps(enabled.unwrap_or(true)),
        };
    }

    /// Combine with another specification holding no second text matcher or style filter.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when the scopes conflict or both hold a text matcher or
    /// style filter.
    #[napi]
    pub fn and(&self, other: &JsLocatorSpec) -> napi::Result<Self> {
        return Self::wrap(self.spec.clone().and(other.spec.clone()));
    }
}

/// One match tied to the exact frame that produced it.
#[napi(js_name = "ScreenMatch")]
#[derive(Clone)]
pub struct JsScreenMatch {
    pub(crate) inner: ScreenMatch,
}

#[napi]
impl JsScreenMatch {
    /// The exact frame evaluated.
    #[napi(getter)]
    pub fn frame(&self) -> JsFrame {
        return JsFrame::new(Arc::clone(&self.inner.frame));
    }

    /// Ordered half-open cell ranges.
    #[napi(getter)]
    pub fn ranges(&self) -> Model<Vec<termtrace_sdk::locator::MatchRange>> {
        return Model(self.inner.ranges.to_vec());
    }

    /// Matched text with spaces and graphemes preserved; `length` counts UTF-16 units, not
    /// terminal columns.
    #[napi(getter)]
    pub fn text(&self) -> String {
        return self.inner.text.to_string();
    }

    /// Covered cells, at most 128.
    #[napi(getter)]
    pub fn cells(&self) -> Model<Vec<termtrace_sdk::locator::MatchedCell>> {
        return Model(self.inner.covered_cells.to_vec());
    }

    /// Covered cells omitted from `cells`.
    #[napi(getter)]
    pub fn omitted_cell_count(&self) -> f64 {
        return f64::from(u32::try_from(self.inner.omitted_cell_count).unwrap_or(u32::MAX));
    }

    /// Bounded normalized query.
    #[napi(getter)]
    pub fn query(&self) -> Model<termtrace_sdk::locator::QueryDescription> {
        return Model(self.inner.query.clone());
    }
}

fn matches(found: &[ScreenMatch]) -> Vec<JsScreenMatch> {
    return found
        .iter()
        .map(|inner| {
            return JsScreenMatch {
                inner: inner.clone(),
            };
        })
        .collect();
}

/// Matches evaluated over one exact frame.
#[napi(js_name = "QueryResult")]
#[derive(Clone)]
pub struct JsQueryResult {
    pub(crate) inner: QueryResult,
}

impl JsQueryResult {
    /// Wrap one evaluation.
    pub fn new(inner: QueryResult) -> Self {
        return Self { inner };
    }
}

#[napi]
impl JsQueryResult {
    /// The exact frame evaluated.
    #[napi(getter)]
    pub fn frame(&self) -> JsFrame {
        return JsFrame::new(Arc::clone(self.inner.frame()));
    }

    /// Every match in row and column order.
    #[napi]
    pub fn all(&self) -> Vec<JsScreenMatch> {
        return matches(self.inner.all());
    }

    /// Exact match count.
    #[napi]
    pub fn count(&self) -> u32 {
        return u32::try_from(self.inner.count()).unwrap_or(u32::MAX);
    }

    /// Require exactly one match.
    ///
    /// # Errors
    ///
    /// Fails with kind `no_match` or `multiple_matches`.
    #[napi]
    pub fn single(&self) -> napi::Result<JsScreenMatch> {
        return self
            .inner
            .single()
            .map(|inner| {
                return JsScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_error(&error));
    }

    /// Select the first match, rejecting absence but not ambiguity.
    ///
    /// # Errors
    ///
    /// Fails with kind `no_match`.
    #[napi]
    pub fn first(&self) -> napi::Result<JsScreenMatch> {
        return self
            .inner
            .first()
            .map(|inner| {
                return JsScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_error(&error));
    }

    /// Select one zero-based match.
    ///
    /// # Errors
    ///
    /// Fails when `index` is not a non-negative integer, and with kind `index_out_of_bounds`.
    #[napi]
    pub fn nth(&self, index: f64) -> napi::Result<JsScreenMatch> {
        let index = self::index(index, "index")?;

        return self
            .inner
            .nth(index)
            .map(|inner| {
                return JsScreenMatch {
                    inner: inner.clone(),
                };
            })
            .map_err(|error| return query_error(&error));
    }
}

/// A satisfied locator wait: the satisfying frame and its matches.
#[napi(js_name = "LocatorWaitResult")]
#[derive(Clone)]
pub struct JsLocatorWaitResult {
    inner: LocatorWaitResult,
}

#[napi]
impl JsLocatorWaitResult {
    /// The exact satisfying frame.
    #[napi(getter)]
    pub fn frame(&self) -> JsFrame {
        return JsFrame::new(Arc::clone(&self.inner.frame));
    }

    /// Matches from that frame; empty for an `absent` condition.
    #[napi(getter)]
    pub fn matches(&self) -> Vec<JsScreenMatch> {
        return matches(&self.inner.matches);
    }
}

/// Native wait bounds; `signal` is handled by the JavaScript facade.
#[napi(object, object_to_js = false, js_name = "NativeLocatorWaitOptions")]
pub struct JsLocatorWaitOptions {
    /// `"exists"`, `"absent"`, or `{ count: n }`.
    pub condition: Option<Arg<LocatorCondition>>,
    /// Overall bound in whole milliseconds.
    pub timeout_ms: Option<f64>,
    /// Pause between polls in whole milliseconds.
    pub interval_ms: Option<f64>,
    /// How long the condition must hold in one unchanged state, in whole milliseconds.
    pub stable_for_ms: Option<f64>,
}

/// Lazy locator bound to a session; every operation captures one fresh frame.
#[napi(js_name = "Locator")]
#[derive(Clone)]
pub struct JsLocator {
    terminal: Terminal,
    session_path: PathBuf,
    spec: LocatorSpec,
}

impl JsLocator {
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

    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Locator<'_>) -> Result<T, LocatorError> + Send + 'static,
    ) -> napi::Result<T> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let spec = self.spec.clone();

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;

            return operation(&session.locator(spec)).map_err(locator_error);
        })
        .await
        .map_err(blocking_failure)?;
    }
}

#[napi]
impl JsLocator {
    /// The bound specification.
    #[napi(getter)]
    pub fn spec(&self) -> JsLocatorSpec {
        return JsLocatorSpec {
            spec: self.spec.clone(),
        };
    }

    /// Combine with another specification without capturing.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when the scopes conflict or both hold a text matcher or
    /// style filter.
    #[napi]
    pub fn and(&self, other: &JsLocatorSpec) -> napi::Result<Self> {
        return self
            .spec
            .clone()
            .and(other.spec.clone())
            .map(|spec| return self.rebind(spec))
            .map_err(|error| return query_error(&error));
    }

    /// Restrict to one absolute row without capturing.
    ///
    /// # Errors
    ///
    /// Fails when `row` is not a whole number from 0 through 65535.
    #[napi]
    pub fn in_row(&self, row: f64) -> napi::Result<Self> {
        let row = crate::host::cell_coordinate(row, "row")?;

        return Ok(self.rebind(self.spec.clone().in_row(row)));
    }

    /// Restrict to one nonempty absolute half-open region without capturing.
    ///
    /// # Errors
    ///
    /// Fails when `region` is malformed, and with kind `invalid_query` when it is empty.
    #[napi]
    pub fn in_region(&self, region: Arg<ScreenRegion>) -> napi::Result<Self> {
        let region = region.0;

        return self
            .spec
            .clone()
            .in_region(region)
            .map(|spec| return self.rebind(spec))
            .map_err(|error| return query_error(&error));
    }

    /// Require a style filter without capturing.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `style` is malformed.
    #[napi]
    pub fn with_style(&self, style: Arg<StyleFilter>) -> Self {
        return self.rebind(self.spec.clone().with_style(style.0));
    }

    /// Let text matches continue across native soft-wrapped rows.
    #[napi]
    pub fn across_soft_wraps(&self, enabled: Option<bool>) -> Self {
        return self.rebind(self.spec.clone().across_soft_wraps(enabled.unwrap_or(true)));
    }

    /// Capture one frame and evaluate it once.
    ///
    /// # Errors
    ///
    /// Fails with a query kind when the specification does not fit the frame, or the capture's
    /// own error.
    #[napi]
    pub async fn evaluate(&self) -> napi::Result<JsQueryResult> {
        return self
            .run(|locator| return locator.evaluate())
            .await
            .map(JsQueryResult::new);
    }

    /// Capture once and return every match.
    ///
    /// # Errors
    ///
    /// Fails like `evaluate`.
    #[napi]
    pub async fn all(&self) -> napi::Result<Vec<JsScreenMatch>> {
        return self
            .run(|locator| return locator.all())
            .await
            .map(|found| return matches(&found));
    }

    /// Capture once and count matches.
    ///
    /// # Errors
    ///
    /// Fails like `evaluate`.
    #[napi]
    pub async fn count(&self) -> napi::Result<u32> {
        return self
            .run(|locator| return locator.count())
            .await
            .map(|count| return u32::try_from(count).unwrap_or(u32::MAX));
    }

    /// Capture once and require exactly one match.
    ///
    /// # Errors
    ///
    /// Fails like `evaluate`, and with kind `no_match` or `multiple_matches`.
    #[napi]
    pub async fn single(&self) -> napi::Result<JsScreenMatch> {
        return self
            .run(|locator| return locator.single())
            .await
            .map(|inner| return JsScreenMatch { inner });
    }

    /// Capture once and select the first match.
    ///
    /// # Errors
    ///
    /// Fails like `evaluate`, and with kind `no_match`.
    #[napi]
    pub async fn first(&self) -> napi::Result<JsScreenMatch> {
        return self
            .run(|locator| return locator.first())
            .await
            .map(|inner| return JsScreenMatch { inner });
    }

    /// Capture once and select one zero-based match.
    ///
    /// # Errors
    ///
    /// Fails when `index` is not a non-negative integer, like `evaluate`, and with kind
    /// `index_out_of_bounds`.
    #[napi]
    pub async fn nth(&self, index: f64) -> napi::Result<JsScreenMatch> {
        let index = crate::host::index(index, "index")?;

        return self
            .run(move |locator| return locator.nth(index))
            .await
            .map(|inner| return JsScreenMatch { inner });
    }

    /// Wait until at least one match exists; `waitFor` without cancellation.
    ///
    /// # Errors
    ///
    /// Fails like `waitFor`.
    #[napi]
    pub async fn wait(
        &self,
        timeout_ms: Option<f64>,
        interval_ms: Option<f64>,
    ) -> napi::Result<JsLocatorWaitResult> {
        let timeout = millis(timeout_ms, defaults::WAIT_TIMEOUT)?;
        let interval = millis(interval_ms, defaults::LOCATOR_POLL_INTERVAL)?;

        return self
            .run(move |locator| return locator.wait(timeout, interval))
            .await
            .map(|inner| return JsLocatorWaitResult { inner });
    }

    /// Wait until at least one match holds in one unchanged state for `stableForMs`.
    ///
    /// # Errors
    ///
    /// Fails like `waitFor`.
    #[napi]
    pub async fn wait_stable(
        &self,
        timeout_ms: Option<f64>,
        interval_ms: Option<f64>,
        stable_for_ms: Option<f64>,
    ) -> napi::Result<JsLocatorWaitResult> {
        let timeout = millis(timeout_ms, defaults::WAIT_TIMEOUT)?;
        let interval = millis(interval_ms, defaults::LOCATOR_POLL_INTERVAL)?;
        let stable_for = millis(stable_for_ms, Duration::ZERO)?;

        return self
            .run(move |locator| return locator.wait_stable(timeout, interval, stable_for))
            .await
            .map(|inner| return JsLocatorWaitResult { inner });
    }

    /// Poll until the condition holds in one unchanged state for `stableForMs`.
    ///
    /// The promise settles only after the native worker returns. Aborting `signal` (or
    /// cancelling `cancel`) settles within about one 250 ms capture quantum plus a round trip
    /// against a responsive session, and within about 5.25 s when the session is busy with
    /// other work. No input is ever sent.
    ///
    /// # Errors
    ///
    /// Fails for malformed durations, with kind `invalid_query` for an invalid condition, zero
    /// timeout or interval, or `stableForMs` above `timeoutMs`, `locator_timeout` with the last
    /// `frame` when unsatisfied in time, `cancelled` with the last `frame` after cancellation,
    /// and the capture's own error otherwise.
    #[napi]
    pub async fn wait_for(
        &self,
        options: Option<JsLocatorWaitOptions>,
        cancel: Option<&JsCancelToken>,
    ) -> napi::Result<JsLocatorWaitResult> {
        let options = options.unwrap_or(JsLocatorWaitOptions {
            condition: None,
            timeout_ms: None,
            interval_ms: None,
            stable_for_ms: None,
        });
        let condition = match options.condition {
            Some(condition) => condition.0,
            None => LocatorCondition::Exists,
        };
        let options = LocatorWaitOptions {
            condition,
            timeout: millis(options.timeout_ms, defaults::WAIT_TIMEOUT)?,
            interval: millis(options.interval_ms, defaults::LOCATOR_POLL_INTERVAL)?,
            stable_for: millis(options.stable_for_ms, Duration::ZERO)?,
            cancel: cancel.map(|cancel| return cancel.inner.clone()),
        };

        return self
            .run(move |locator| return locator.wait_for(options))
            .await
            .map(|inner| return JsLocatorWaitResult { inner });
    }
}
