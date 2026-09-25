//! Lazy session-bound locators over pure rich-frame queries.

use super::query::{
    query_frame, LocatorSpec, QueryDescription, QueryError, QueryResult, ScreenMatch, StyleFilter,
};
use crate::{
    client::{Screen, Session},
    screen::StableCaptureOptions,
};
use regex::Regex;
use std::{
    error::Error as StdError,
    fmt,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion,
};

impl<'session> Screen<'session> {
    /// Bind an immutable specification to this screen without performing I/O.
    #[must_use]
    pub fn locator(&self, spec: LocatorSpec) -> Locator<'session> {
        return Locator::new(self.session(), spec);
    }

    /// Bind a nonempty literal text query without performing I/O.
    pub fn get_by_text(&self, text: impl Into<String>) -> Result<Locator<'session>, QueryError> {
        return Ok(self.locator(LocatorSpec::text(text)?));
    }

    /// Bind an already compiled host-native regular expression without performing I/O.
    pub fn get_by_regex(&self, regex: Regex) -> Result<Locator<'session>, QueryError> {
        return Ok(self.locator(LocatorSpec::regex(regex)?));
    }

    /// Bind one absolute row scope without performing I/O.
    #[must_use]
    pub fn row(&self, row: u16) -> Locator<'session> {
        return self.locator(LocatorSpec::default().in_row(row));
    }

    /// Bind one exact absolute region without performing I/O.
    pub fn region(&self, region: ScreenRegion) -> Result<Locator<'session>, QueryError> {
        return Ok(self.locator(LocatorSpec::default().in_region(region)?));
    }

    /// Bind a complete appearance filter without performing I/O.
    #[must_use]
    pub fn filter(&self, style: StyleFilter) -> Locator<'session> {
        return self.locator(LocatorSpec::default().with_style(style));
    }
}

/// Lazy session-bound immutable locator.
#[derive(Clone)]
pub struct Locator<'session> {
    session: &'session Session,
    spec: LocatorSpec,
}

impl<'session> Locator<'session> {
    /// Bind an immutable specification without performing I/O.
    #[must_use]
    pub fn new(session: &'session Session, spec: LocatorSpec) -> Self {
        return Self { session, spec };
    }

    /// Compose another pure specification without performing I/O.
    pub fn and(&self, other: LocatorSpec) -> Result<Self, QueryError> {
        return Ok(Self {
            session: self.session,
            spec: self.spec.clone().and(other)?,
        });
    }

    /// Capture exactly one latest-presented frame and evaluate it once.
    pub fn evaluate(&self) -> Result<QueryResult, LocatorError> {
        let frame = self
            .session
            .screen()
            .frame(ScreenProjectionRequest {
                detail: FrameDetail::Spans,
                region: self.spec.region(),
            })
            .map_err(LocatorError::Capture)?;

        return query_frame(Arc::new(frame), self.spec.clone()).map_err(LocatorError::Query);
    }

    /// Evaluate all matches after exactly one capture.
    pub fn all(&self) -> Result<Arc<[ScreenMatch]>, LocatorError> {
        return Ok(self.evaluate()?.matches);
    }

    /// Count matches after exactly one capture.
    pub fn count(&self) -> Result<usize, LocatorError> {
        return Ok(self.evaluate()?.count());
    }

    /// Strictly require one match after exactly one capture.
    pub fn single(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .single()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select the first match after exactly one capture.
    pub fn first(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .first()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select one zero-based match after exactly one capture.
    pub fn nth(&self, index: usize) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .nth(index)
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Poll exact one-frame evaluations and return the satisfying frame without recapture.
    pub fn wait(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        return self.wait_stable(timeout, interval, Duration::ZERO);
    }

    /// Poll until a query remains satisfied in one unchanged semantic state.
    pub fn wait_stable(
        &self,
        timeout: Duration,
        interval: Duration,
        stable_for: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        if timeout.is_zero() || interval.is_zero() {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout and interval must be greater than zero".to_owned(),
            }));
        }

        if stable_for > timeout {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator stable interval must not exceed its timeout".to_owned(),
            }));
        }

        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            return LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout exceeds the monotonic clock range".to_owned(),
            });
        })?;

        let mut matching_state: Option<(String, Instant)> = None;
        let mut last_frame: Option<Arc<ScreenFrame>> = None;

        loop {
            let before_capture = Instant::now();
            let Some(remaining) = deadline.checked_duration_since(before_capture) else {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: last_frame,
                    query: self.spec.description(),
                });
            };
            let capture_timeout = remaining.min(Duration::from_secs(3_600));
            let capture = self.session.screen().capture_stable(StableCaptureOptions {
                stable_for: Duration::from_nanos(1).min(capture_timeout),
                timeout: capture_timeout,
                projection: ScreenProjectionRequest {
                    detail: FrameDetail::Spans,
                    region: self.spec.region(),
                },
                allow_incomplete: true,
            });

            let observed_at = Instant::now();
            let capture = match capture {
                Ok(capture) => capture,
                Err(error)
                    if observed_at >= deadline
                        || matches!(
                            &error,
                            crate::Error::Control(termtrace_control::Error::Timeout { .. })
                        ) =>
                {
                    return Err(LocatorError::Timeout {
                        timeout,
                        frame: last_frame,
                        query: self.spec.description(),
                    });
                }
                Err(error) => return Err(LocatorError::Capture(error)),
            };

            let frame = Arc::new(capture.frame);
            last_frame = Some(Arc::clone(&frame));

            if observed_at >= deadline {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: last_frame,
                    query: self.spec.description(),
                });
            }

            let result = query_frame(frame, self.spec.clone()).map_err(LocatorError::Query)?;
            let evaluated_at = Instant::now();

            if evaluated_at >= deadline {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: Some(result.frame),
                    query: self.spec.description(),
                });
            }

            if !result.matches.is_empty() {
                if stable_for.is_zero() {
                    return Ok(LocatorWaitResult {
                        frame: result.frame,
                        matches: result.matches,
                    });
                }

                match &matching_state {
                    Some((state_hash, since)) if state_hash == &result.frame.state_hash => {
                        if evaluated_at.saturating_duration_since(*since) >= stable_for {
                            return Ok(LocatorWaitResult {
                                frame: result.frame,
                                matches: result.matches,
                            });
                        }
                    }
                    _ => matching_state = Some((result.frame.state_hash.clone(), evaluated_at)),
                }
            } else {
                matching_state = None;
            }

            thread::sleep(interval.min(deadline.saturating_duration_since(evaluated_at)));
        }
    }
}

/// Successful locator wait evidence from the same satisfying frame.
#[derive(Clone, Debug)]
pub struct LocatorWaitResult {
    /// Exact satisfying frame.
    pub frame: Arc<ScreenFrame>,
    /// Matches evaluated from that same frame.
    pub matches: Arc<[ScreenMatch]>,
}

/// Session-bound locator infrastructure and pure-query failures.
#[derive(Debug)]
pub enum LocatorError {
    /// Exact rich frame capture failed.
    Capture(crate::Error),
    /// Pure evaluation failed.
    Query(QueryError),
    /// The monotonic locator deadline elapsed.
    Timeout {
        /// Requested bound.
        timeout: Duration,
        /// Last exact frame captured before the deadline, when one was available.
        frame: Option<Arc<ScreenFrame>>,
        /// Bounded normalized query.
        query: QueryDescription,
    },
}

impl fmt::Display for LocatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capture(error) => return write!(f, "capture locator frame: {error}"),
            Self::Query(error) => return error.fmt(f),
            Self::Timeout { timeout, query, .. } => {
                return write!(f, "locator {} timed out after {timeout:?}", query.text);
            }
        }
    }
}

impl StdError for LocatorError {}
